#!/usr/bin/env python3
"""The outlier cohort (FH.1a): the standing corpus a glia release is judged on. Real public
repos chosen for the shape they bring (repos.json), and tasks with known answers mined from
the authors' own history (tasks/<label>.jsonl).

    python3 cohort.py build-tasks [--only <label>[,..]] [--per-kind 4] [--max-commits 3000]
                                  [--jobs 4] [--refetch]
    python3 cohort.py build-tasks --check [--only ..]
    python3 cohort.py starts [--only ..] [--compare-agent-ab]

Task kinds, newest first per repo (rows in bench/agent-ab/tasks.py's schema plus "shape"):

  drift    a commit that changed a server side and a client side together (2..10 non-test
           code files, at least one on each side; a generated path counts as the server's,
           as a regenerated SDK follows the server). The task starts at a SYNTHETIC commit S:
           the real parent P plus every non-client change of the commit C, so the checkout
           holds the server half and the truth is the authors' client half (S..C). S is never
           committed anywhere upstream, so a row holds its recipe (`start`: C, P, the client
           prefixes and the generated-path rule) and its `parent` is the ref
           refs/cohort/drift/<id> that `starts` (or build-tasks) writes into the bare clone;
           the rebuild is byte-identical (fixed identity and dates).
  ripple   a definition whose name or signature moves in one file and is edited in >= 2
           other code files (its callers); the prompt is the commit subject.
  symptom  a commit that closes a GitHub issue; the prompt is the issue alone (title + body,
           redacted), never the commit subject that states the fix.

A repo row's `picks` names commits taken first, whatever the filters say (the 13 FE.4 drift
tasks of bench/agent-ab/probe/drift.py); mined rows fill each kind up to --per-kind. A
`library` row has no boundary sides, so it gets no drift task (its role is judged by FH.2,
never on client reach).

Bare clones live under ${GLIA_COHORT_DIR:-~/.cache/glia-cohort}/<label>.git; a missing clone is
made from ${GLIA_AGENT_AB_DIR:-~/.cache/glia-agent-ab}/<label>.git with `git clone --local`
when that exists, else from the url. Nothing is written in the glia tree but tasks/*.jsonl.
GLIA_COHORT_OFFLINE=1: no clone, fetch or `gh` call. Issue text already in a task file is
carried over (never refetched) unless --refetch, so a rebuild is stable; --check re-derives
every row offline and exits 1 naming each differing id. Per repo it prints
`[cohort] tasks repo=<label> drift=<d> ripple=<r> symptom=<s> starts=<n>`. Stdlib only.
"""
import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "agent-ab"))
import tasks as T  # noqa: E402

KINDS = ("drift", "ripple", "symptom")
ROLES = ("app", "library", "service-set")
PER_KIND, MAX_PER_KIND, MIN_PER_KIND, MAX_COMMITS = 4, 6, 3, 3000
# probe/drift.py's generated-path rule, verbatim: a repo's `generated` adds alternatives.
GEN_DEFAULT = (r"(openapi|generated|\.g\.dart$|\.freezed\.dart$|/gen/|typescript-sdk"
               r"|__generated__|\.pb\.)")
CODE_EXTS = T.CODE_EXTS | {".svelte"}
DRIFT_FILES = (2, 10)
MINED_FILES, MINED_LINES = (2, 12), (5, 600)
START_REF = "refs/cohort/drift/"
IDENTITY = {"GIT_AUTHOR_NAME": "glia-cohort", "GIT_AUTHOR_EMAIL": "cohort@example.invalid",
            "GIT_COMMITTER_NAME": "glia-cohort", "GIT_COMMITTER_EMAIL": "cohort@example.invalid"}
DRIFT_PROMPT = ("The latest commit on this branch (HEAD) changed the server: {subject}. The "
                "{names} were not updated with it and no longer match the server. Update the "
                "client code so it works with the server as it is now.")
SYMPTOM_PROMPT = "Resolve the GitHub issue below."
HEX40 = re.compile(r"^[0-9a-f]{40}$")
DEF_RE = re.compile(
    r"^[-+]\s*(?:export\s+)?(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?"
    r"(?:func(?:\s*\([^)]*\))?|def|function|fn|fun|class|interface|"
    r"(?:public|private|protected|static|final|override|\s)+[\w<>\[\],.? ]+?)\s+"
    r"([A-Za-z_]\w{3,})\s*[(<]")
SKIP_NAMES = {"main", "init", "test", "setup", "self", "this", "func", "function", "return"}


class CohortError(Exception):
    pass


# --- repos ------------------------------------------------------------------
def offline():
    return os.environ.get("GLIA_COHORT_OFFLINE") == "1"


def cohort_dir():
    return Path(os.environ.get("GLIA_COHORT_DIR") or Path.home() / ".cache" / "glia-cohort")


def validate(repo):
    label = repo.get("label")
    for key in ("label", "url", "sha", "lang", "role", "sides", "shape", "public"):
        if key not in repo:
            raise CohortError(f"repos.json row {label!r}: missing {key}")
    if repo["public"] is not True:
        raise CohortError(f"{label}: refused, not marked public: true")
    if not repo["url"].startswith("https://github.com/"):
        raise CohortError(f"{label}: refused, url is not a github https url")
    if not HEX40.match(repo["sha"]):
        raise CohortError(f"{label}: sha is not 40 hex")
    if repo["role"] not in ROLES:
        raise CohortError(f"{label}: role {repo['role']!r} is not one of {ROLES}")
    sides = repo["sides"]
    for key in ("server", "client"):
        if any(not p.endswith("/") for p in sides.get(key, [])):
            raise CohortError(f"{label}: a {key} prefix does not end in /")
    has = bool(sides.get("server")) and bool(sides.get("client"))
    if (repo["role"] == "library") == has:
        raise CohortError(f"{label}: a library has no sides, an app / service-set both")
    if has and not sides.get("client_names"):
        raise CohortError(f"{label}: sides need client_names (the drift prompt's words)")
    for kind, shas in repo.get("picks", {}).items():
        if kind not in KINDS or any(not HEX40.match(s) for s in shas):
            raise CohortError(f"{label}: picks.{kind} must be full shas of a known kind")


def load_repos(path=HERE / "repos.json"):
    repos = json.loads(Path(path).read_text())["repos"]
    seen = set()
    for r in repos:
        validate(r)
        if r["label"] in seen:
            raise CohortError(f"{r['label']}: label repeated")
        seen.add(r["label"])
    return repos


def has_commit(gd, rev):
    return T.git(gd, "cat-file", "-e", f"{rev}^{{commit}}", check=False).returncode == 0


def ensure_clone(repo):
    gd = cohort_dir() / f"{repo['label']}.git"
    if not gd.exists():
        if offline():
            raise CohortError(f"{repo['label']}: no bare clone at {gd} (offline)")
        gd.parent.mkdir(parents=True, exist_ok=True)
        local = T.cache_dir() / f"{repo['label']}.git"
        src = [str(local), "--local"] if local.exists() else [repo["url"]]
        subprocess.run(["git", "clone", "--bare", "--quiet", *src, str(gd)], check=True)
    if not has_commit(gd, repo["sha"]):
        if not offline():
            T.git(gd, "fetch", "--quiet", repo["url"], repo["sha"], check=False)
        if not has_commit(gd, repo["sha"]):
            raise CohortError(f"{repo['label']}: cannot resolve {repo['sha']} in {gd}")
    return gd


# --- sides and the synthetic start ----------------------------------------
def gen_rule(repo):
    extra = repo["sides"].get("generated")
    return f"(?:{GEN_DEFAULT})|(?:{extra})" if extra else GEN_DEFAULT


def is_client(prefixes, gen, path):
    return path.startswith(tuple(prefixes)) and not gen.search(path)


def is_code(path):
    return os.path.splitext(path)[1] in CODE_EXTS


def client_paths(files, prefixes, gen):
    """Every path of parent..commit the client half owns (new and, for a rename, old)."""
    out = set()
    for _, path, old, _ in files:
        out |= {p for p in (path, old) if p and is_client(prefixes, gen, p)}
    return out


def git_out(gd, *args, env=None, inp=None):
    p = subprocess.run(["git", "--git-dir", str(gd), "-c", "commit.gpgSign=false", *args],
                       capture_output=True, text=True, input=inp,
                       env={**os.environ, "LC_ALL": "C", **(env or {})})
    if p.returncode:
        raise CohortError(f"git {' '.join(args[:2])}: {p.stderr.strip()}")
    return p.stdout.strip()


def build_start(gd, row):
    """Write S (C's tree with every client path put back as it was at P) as the row's ref and
    return its sha. Deterministic: same recipe, same bytes, in any clone of the repo."""
    st = row["start"]
    commit, parent = st["commit"], st["real_parent"]
    paths = client_paths(T.changed_files(gd, parent, commit), st["client_prefixes"],
                         re.compile(st["generated"], re.I))
    subject = git_out(gd, "log", "-1", "--format=%s", commit)
    when = git_out(gd, "log", "-1", "--format=%at", commit)
    with tempfile.TemporaryDirectory() as t:
        env = {"GIT_INDEX_FILE": str(Path(t) / "index"), **IDENTITY,
               "GIT_AUTHOR_DATE": f"@{when} +0000", "GIT_COMMITTER_DATE": f"@{when} +0000"}
        git_out(gd, "read-tree", commit, env=env)
        for path in sorted(paths):
            at_p = git_out(gd, "ls-tree", parent, "--", path)
            if at_p:
                meta, _, _ = at_p.partition("\t")
                mode, _, sha = meta.split(" ")
                git_out(gd, "update-index", "--add", "--cacheinfo", f"{mode},{sha},{path}",
                        env=env)
            else:  # the commit added it: drop it (mode 0 removes, no work tree needed)
                git_out(gd, "update-index", "--index-info", env=env,
                        inp=f"0 {'0' * 40}\t{path}\n")
        tree = git_out(gd, "write-tree", env=env)
        start = git_out(gd, "commit-tree", tree, "-p", parent, "-m", f"server: {subject}",
                        env=env)
    git_out(gd, "update-ref", st["ref"], start)
    return start


def checkout_rev(gd, row):
    """The rev a task starts at: a drift row's synthetic start (rebuilt when its ref is
    missing), else the row's parent."""
    if row["shape"] != "drift":
        return row["parent"]
    if has_commit(gd, row["start"]["ref"]):
        return git_out(gd, "rev-parse", row["start"]["ref"])
    return build_start(gd, row)


# --- mining -----------------------------------------------------------------
def truth_of(gd, a, b, files):
    code = [f for f in files if is_code(f[1]) and not T.is_test(f[1])]
    truth = {"code": [{"path": p, "status": s, "old_path": o, "hunks": T.hunks(gd, a, b, p, o)}
                      for s, p, o, _ in sorted(code, key=lambda f: f[1])],
             "tests": sorted(f[1] for f in files if T.is_test(f[1])),
             "other": sorted(f[1] for f in files if f not in code and not T.is_test(f[1]))}
    stats = {"code_files": len(code), "code_lines": sum(f[3] for f in code),
             "test_files": len(truth["tests"])}
    return truth, stats


def diff_text(gd, parent, commit, path, old):
    return T.git(gd, "diff", "--no-ext-diff", "--no-textconv", "-M", "-U0", parent, commit,
                 "--", *([old] if old else []), path).stdout.decode("utf-8", errors="replace")


def ripple(gd, parent, commit, code):
    """probe/mine.py's rule: the best (name, defining file, caller files) or None."""
    texts = {f[1]: diff_text(gd, parent, commit, f[1], f[2]) for f in code}
    best = None
    for path, text in sorted(texts.items()):
        names = sorted({m.group(1) for line in text.splitlines() if line[:1] in "+-"
                        for m in [DEF_RE.match(line)]
                        if m and m.group(1).lower() not in SKIP_NAMES})
        for name in names:
            word = re.compile(rf"\b{re.escape(name)}\b")
            callers = sorted(p for p, t in texts.items() if p != path and any(
                line[:1] in "+-" and word.search(line) for line in t.splitlines()))
            if len(callers) >= 2 and (best is None or len(callers) > len(best[2])):
                best = (name, path, callers)
    return best


def base_row(repo, commit, parent, when, subject, prompt, pr, shape):
    return {"id": f"{repo['label']}-{commit[:12]}", "repo": repo["label"], "commit": commit,
            "parent": parent, "author_time": when, "subject": T.redact(subject),
            "prompt_subject": prompt, "pr": pr, "issue": None, "issue_fetch": "none",
            "after_gate_rev": None, "shape": shape}


def drift_row(repo, gd, commit, parent, when, subject, pr, files):
    """The drift row of commit, or None when it does not split into two non-empty halves."""
    sides = repo["sides"]
    gen = re.compile(gen_rule(repo), re.I)
    client = client_paths(files, sides["client"], gen)
    if not client or len(client) == len({p for f in files for p in (f[1], f[2]) if p}):
        return None
    row = base_row(repo, commit, None, when, subject, DRIFT_PROMPT.format(
        subject=T.redact(T.PREFIX_RE.sub("", subject, count=1)),
        names=sides["client_names"]), pr, "drift")
    row["start"] = {"ref": START_REF + row["id"], "commit": commit, "real_parent": parent,
                    "client_prefixes": list(sides["client"]), "generated": gen_rule(repo)}
    row["parent"] = row["start"]["ref"]
    start = build_start(gd, row)
    row["truth"], row["stats"] = truth_of(gd, start, commit, T.changed_files(gd, start, commit))
    row["probe"] = {"client_files": sorted(client), "real_parent": parent,
                    "server_files": sorted(f[1] for f in files if f[1] not in client)}
    return row


def drift_eligible(repo, files):
    sides = repo["sides"]
    if not sides.get("client"):
        return False
    gen = re.compile(gen_rule(repo), re.I)
    code = [f for f in files if is_code(f[1]) and not T.is_test(f[1])]
    if not DRIFT_FILES[0] <= len(code) <= DRIFT_FILES[1] or \
            sum(f[3] for f in code) < MINED_LINES[0]:
        return False
    client = [f for f in code if is_client(sides["client"], gen, f[1])]
    server = [f for f in code if not is_client(sides["client"], gen, f[1])
              and (f[1].startswith(tuple(sides["server"])) or gen.search(f[1]))]
    return bool(client) and bool(server)


def mined_size_ok(files):
    code = [f for f in files if is_code(f[1]) and not T.is_test(f[1])]
    lines = sum(f[3] for f in files if is_code(f[1]))
    return (MINED_FILES[0] <= len(code) <= MINED_FILES[1]
            and MINED_LINES[0] <= lines <= MINED_LINES[1]
            and any(f[0] in "MR" for f in code)), code


def entry_of(gd, commit):
    out = T.git(gd, "log", "-1", "--format=%H%x00%P%x00%an%x00%ae%x00%cn%x00%ce%x00%at%x00%B",
                commit).stdout.decode("utf-8", errors="replace")
    h, parents, an, ae, cn, ce, at, body = out.split("\x00", 7)
    return {"commit": h, "parents": parents.split(), "people": (an, ae, cn, ce),
            "author_time": int(at), "message": body.rstrip("\n").replace("\r\n", "\n")}


def pick_row(repo, gd, kind, sha):
    """A picked commit's row, whatever the mining filters say (its first parent is P)."""
    e = entry_of(gd, sha)
    if not e["parents"]:
        raise CohortError(f"{repo['label']}: pick {sha} has no parent")
    subject = e["message"].split("\n")[0].strip()
    m = T.MERGE_PR_RE.match(subject)
    pr = int(m.group(1)) if m else None
    files = T.changed_files(gd, e["parents"][0], sha)
    if kind == "drift":
        row = drift_row(repo, gd, sha, e["parents"][0], e["author_time"], subject, pr, files)
        if row is None:
            raise CohortError(f"{repo['label']}: drift pick {sha} has no client half")
        return row
    raise CohortError(f"{repo['label']}: picks.{kind} is not supported (drift only)")


def fetch_issue(url, refs):
    """tasks.py's issue fetch (`gh api`, redacted, PRs skipped); nothing when offline."""
    return (None, "unavailable") if offline() else T.fetch_issue(url, refs)


def mine(repo, gd, per_kind=PER_KIND, max_commits=MAX_COMMITS, carried=None,
         fetch=fetch_issue):
    """The repo's task rows. `carried` maps id -> (issue, issue_fetch) of rows already
    committed; a symptom candidate not in it is fetched unless carried is strict (check)."""
    rows, used = {k: [] for k in KINDS}, set()
    for kind in KINDS:
        for sha in repo.get("picks", {}).get(kind, []):
            rows[kind].append(pick_row(repo, gd, kind, sha))
            used.add(sha)
    cap = {k: max(per_kind, len(rows[k])) for k in KINDS}
    strict = carried is not None and carried.get("__strict__", False)
    for i, e in enumerate(T.log_first_parent(gd, repo["sha"])):
        if i >= max_commits or all(len(rows[k]) >= cap[k] for k in KINDS):
            break
        if e["commit"] in used:
            continue
        c = T.candidate(e)
        if not c:
            continue
        parent, subject, prompt, pr, ref_text = c
        files = T.changed_files(gd, parent, e["commit"])
        if len(rows["drift"]) < cap["drift"] and drift_eligible(repo, files):
            row = drift_row(repo, gd, e["commit"], parent, e["author_time"], subject, pr, files)
            if row:
                rows["drift"].append(row)
                used.add(e["commit"])
                continue
        ok, code = mined_size_ok(files)
        if not ok:
            continue
        rid = f"{repo['label']}-{e['commit'][:12]}"
        if len(rows["symptom"]) < cap["symptom"] and T.CLOSING_RE.search(ref_text):
            if carried and rid in carried:
                issue, fetched = carried[rid]
            elif strict:
                issue, fetched = None, "none"
            else:
                issue, fetched = fetch(repo["url"], T.issue_refs(ref_text))
            truth, stats = truth_of(gd, parent, e["commit"], files)
            if issue and not T.names_a_file(issue["title"], truth):
                row = base_row(repo, e["commit"], parent, e["author_time"], subject,
                               SYMPTOM_PROMPT, pr, "symptom")
                row.update(issue=issue, issue_fetch=fetched, truth=truth, stats=stats,
                           probe={})
                rows["symptom"].append(row)
                used.add(e["commit"])
                continue
        if len(rows["ripple"]) < cap["ripple"]:
            r = ripple(gd, parent, e["commit"], code)
            if r:
                row = base_row(repo, e["commit"], parent, e["author_time"], subject,
                               T.redact(prompt), pr, "ripple")
                row["truth"], row["stats"] = truth_of(gd, parent, e["commit"], files)
                row["probe"] = {"symbol": r[0], "defined_in": r[1], "callers": r[2]}
                rows["ripple"].append(row)
                used.add(e["commit"])
    out = []
    for kind in KINDS:
        out += sorted(rows[kind], key=lambda r: (-r["author_time"], r["id"]))
    return out


# --- files ------------------------------------------------------------------
def render(rows):
    return "".join(json.dumps(r, sort_keys=True, ensure_ascii=False) + "\n" for r in rows)


def marker(label, rows, starts):
    n = {k: sum(r["shape"] == k for r in rows) for k in KINDS}
    print(f"[cohort] tasks repo={label} drift={n['drift']} ripple={n['ripple']} "
          f"symptom={n['symptom']} starts={starts}", flush=True)


def read_rows(path):
    return [json.loads(x) for x in Path(path).read_text().splitlines()] \
        if Path(path).exists() else []


def build_one(repo, out, a):
    gd = ensure_clone(repo)
    path = out / f"{repo['label']}.jsonl"
    old = read_rows(path)
    carried = {r["id"]: (r["issue"], r["issue_fetch"]) for r in old if r.get("issue")}
    if a.check:
        carried["__strict__"] = True
        fresh = mine(repo, gd, a.per_kind, a.max_commits, carried)
        bad = [rid for rid in sorted({r["id"] for r in fresh} | {r["id"] for r in old})
               if next((r for r in fresh if r["id"] == rid), None)
               != next((r for r in old if r["id"] == rid), None)]
        if not bad and render(fresh) != (path.read_text() if path.exists() else ""):
            bad.append(f"{repo['label']} (row order or rendering)")
        rows = fresh
    else:
        rows = mine(repo, gd, a.per_kind, a.max_commits, None if a.refetch else carried)
        out.mkdir(parents=True, exist_ok=True)
        path.write_text(render(rows))
        bad = []
    return repo["label"], rows, bad


def cmd_build(a, repos):
    out, bad, total = Path(a.out), [], 0
    roles = {r["label"]: r["role"] for r in repos}
    with ThreadPoolExecutor(max_workers=max(1, a.jobs)) as pool:
        for label, rows, b in pool.map(lambda r: build_one(r, out, a), repos):
            marker(label, rows, sum(r["shape"] == "drift" for r in rows))
            kinds = [k for k in KINDS if not (k == "drift" and roles[label] == "library")]
            short = [k for k in kinds if sum(r["shape"] == k for r in rows) < MIN_PER_KIND]
            if short:
                print(f"[cohort] short repo={label} kinds={','.join(short)} "
                      f"(history holds fewer than {MIN_PER_KIND} in the first "
                      f"{a.max_commits} commits)", flush=True)
            bad += b
            total += len(rows)
    digest = hashlib.sha256("".join(
        render(read_rows(out / f"{r['label']}.jsonl")) for r in repos).encode()).hexdigest()
    print(f"[cohort] tasks total={total} repos={len(repos)} sha256={digest[:12]}")
    for rid in bad:
        print(f"[cohort] check differs: {rid}", file=sys.stderr)
    return 1 if bad else 0


def cmd_starts(a, repos):
    """Rebuild every drift row's synthetic start; with --compare-agent-ab, compare its tree to
    probe/drift.py's refs/agent-ab/drift/<id> in the agent-ab clone (FE.4's 13 tasks)."""
    bad, compared = [], 0
    for repo in repos:
        gd = ensure_clone(repo)
        rows = [r for r in read_rows(Path(a.out) / f"{repo['label']}.jsonl")
                if r["shape"] == "drift"]
        for r in rows:
            tree = git_out(gd, "rev-parse", build_start(gd, r) + "^{tree}")
            line = f"[cohort] start {r['id']} tree={tree[:12]}"
            if a.compare_agent_ab:
                ab = T.cache_dir() / f"{repo['label']}.git"
                ref = f"refs/agent-ab/drift/{r['id']}"
                if ab.exists() and has_commit(ab, ref):
                    want = git_out(ab, "rev-parse", ref + "^{tree}")
                    compared += 1
                    line += f" agent_ab={'same' if want == tree else 'DIFFERS'}"
                    if want != tree:
                        bad.append(r["id"])
            print(line, flush=True)
        marker(repo["label"], read_rows(Path(a.out) / f"{repo['label']}.jsonl"), len(rows))
    if a.compare_agent_ab:
        print(f"[cohort] starts compared={compared} differ={len(bad)}")
    return 1 if bad else 0


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("cmd", choices=["build-tasks", "starts"])
    ap.add_argument("--only", help="comma-separated labels")
    ap.add_argument("--per-kind", type=int, default=PER_KIND)
    ap.add_argument("--max-commits", type=int, default=MAX_COMMITS)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--check", action="store_true", help="re-derive offline, compare")
    ap.add_argument("--refetch", action="store_true", help="fetch every issue again")
    ap.add_argument("--compare-agent-ab", action="store_true")
    ap.add_argument("--repos", default=str(HERE / "repos.json"), help=argparse.SUPPRESS)
    ap.add_argument("--out", default=str(HERE / "tasks"), help=argparse.SUPPRESS)
    a = ap.parse_args(argv)
    if not 1 <= a.per_kind <= MAX_PER_KIND:
        ap.error(f"--per-kind must be 1..{MAX_PER_KIND}")
    try:
        repos = load_repos(a.repos)
        if a.only:
            want = a.only.split(",")
            repos = [r for r in repos if r["label"] in want]
            if len(repos) != len(set(want)):
                raise CohortError(f"unknown label in --only {a.only!r}")
        if a.check:
            os.environ["GLIA_COHORT_OFFLINE"] = "1"
            os.environ["GLIA_AGENT_AB_OFFLINE"] = "1"
        return cmd_build(a, repos) if a.cmd == "build-tasks" else cmd_starts(a, repos)
    except (CohortError, T.TaskError, subprocess.CalledProcessError, OSError, ValueError) as e:
        print(f"[cohort] error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
