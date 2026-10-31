//! GA.1: the surface replay, the measured experiment (S4) that sizes the
//! body-only watch fast path per language.
//!
//! For the last `--commits` first-parent commits of `--repo` at `--rev`, every
//! MODIFIED source file (status `M`; added, deleted and renamed files are
//! left out) is parsed before and after with `glia_engine::parse_one` and its
//! declaration surface compared through
//! `glia_engine::cache::{surface_fingerprint, surface_fingerprint_strict}`.
//! An edit is body-only when the loose surface is equal and the file holds no
//! body fact another file's resolution reads (a Go `MountArg` today).
//!
//! Two controls run in the same replay: the null control parses up to 20
//! unmodified source files per commit twice and fails the run on any
//! fingerprint inequality; the positive control lists every modified file
//! whose diff adds or removes a top-level declaration line yet keeps the loose
//! surface, each with an explanation or as `unexplained` (which fails the
//! controls).
//!
//! Then, on a `git archive` copy of `--rev` under `--work`, a
//! `glia_engine::watch::WatchSession` times three body-only updates per build
//! group that had edits (one comment line inserted at the top of one of its
//! files, then removed, then inserted), keeping the updates that rebuilt
//! exactly that group, and records the median of each `phase_ms` field.
//!
//! Output (stdout), one line per (repo, group):
//! `[surface-replay] repo=<r> lang=<l> edits=<n> body_only=<loose>/<strict>
//! group_ms=<g> update_ms=<u> build_share=<b> clone_ms=<c> saving_ms=<s>
//! controls=<ok|fail> verdict=<PASS|FAIL> load=<l> threads=<t>` and
//! `[surface-replay] decomp repo=<r> lang=<l> group_ms=<g> grafts_ms=<a>
//! tail_ms=<resolve+post+finalize> delta_ms=<d> clone_ms=<c> update_ms=<u>
//! threads=<t>`; `--out` gets the rows, the controls and every violation as
//! JSON. The pass rule is `dev-notes/surface-replay-0.5.6.md`'s.
//!
//! `cargo run --release -p glia-engine --example surface_replay -- --repo
//! <git dir> --label <name> --commits 200 --rev <rev> --work <dir> --out <json>`
//! (`--no-watch` skips the timing). git runs here, in an example, never in a
//! build.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use glia_code_domain::{FileParse, NavFact, walk_gating};
use glia_core::RepoId;
use glia_engine::cache as glia_cache;
use glia_engine::cache::{surface_fingerprint, surface_fingerprint_strict, surface_language};
use glia_engine::watch::{WatchPhases, WatchSession, WatchUpdate};
use serde_json::{Value, json};

/// The pass rule's bars (`dev-notes/surface-replay-0.5.6.md`).
const MIN_EDITS: usize = 30;
const MIN_BODY_ONLY: f64 = 0.40;
const MIN_GROUP_MS: f64 = 250.0;
const MIN_BUILD_SHARE: f64 = 0.25;
/// Unmodified files the null control parses twice per commit.
const NULL_PER_COMMIT: usize = 20;
/// Timed updates per group.
const UPDATES: usize = 3;

struct Args {
    repo: PathBuf,
    label: String,
    commits: usize,
    rev: String,
    out: Option<PathBuf>,
    work: Option<PathBuf>,
    watch: bool,
}

fn args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut a = Args {
        repo: PathBuf::new(),
        label: String::new(),
        commits: 200,
        rev: "HEAD".into(),
        out: None,
        work: None,
        watch: true,
    };
    while let Some(k) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{k} needs a value"));
        match k.as_str() {
            "--repo" => a.repo = PathBuf::from(val()?),
            "--label" => a.label = val()?,
            "--commits" => a.commits = val()?.parse().map_err(|e| format!("--commits: {e}"))?,
            "--rev" => a.rev = val()?,
            "--out" => a.out = Some(PathBuf::from(val()?)),
            "--work" => a.work = Some(PathBuf::from(val()?)),
            "--no-watch" => a.watch = false,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.repo.as_os_str().is_empty() {
        return Err("--repo is required".into());
    }
    if a.label.is_empty() {
        a.label = a
            .repo
            .file_name()
            .map(|n| n.to_string_lossy().trim_end_matches(".git").to_string())
            .unwrap_or_else(|| "repo".into());
    }
    Ok(a)
}

/// `git -C <repo> <args>` stdout as bytes; Err on a non-zero exit.
fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .map_err(|e| format!("git {args:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String, String> {
    String::from_utf8(git(repo, args)?).map_err(|e| format!("git {args:?}: {e}"))
}

/// The build group of a language tag, as the engine groups them (the TS
/// family is one graph, Kotlin joins Java's): the `groups_rebuilt` key.
fn group_of(lang: &str) -> &str {
    match lang {
        "angular" | "react" | "typescript" | "vue" => "typescript",
        "kotlin" => "java",
        other => other,
    }
}

/// Tags a language parser reads (not the routed schema / config tags).
fn is_code(lang: &str) -> bool {
    !matches!(lang, "proto" | "graphql" | "avro")
}

fn parse(src: &str, path: &str, lang: &str) -> Option<FileParse> {
    glia_engine::parse_one(src, path, lang, RepoId(1)).ok()
}

/// Whether the repo walk reads `path` at all: no directory of it is
/// hard-skipped (`.git`, `.glia`, ..) or always collapsed to a region
/// (`node_modules`, `vendor`, `build`, `dist`, `target`, ..; `gate_dir`'s
/// name rules). An edit the walk never reads costs a watch update nothing.
/// `.gitignore`d directories are not checked: a tracked file rarely sits in one.
fn walked(path: &str) -> bool {
    let mut dirs: Vec<&str> = path.split('/').collect();
    dirs.pop();
    !dirs
        .iter()
        .any(|d| walk_gating::is_hard_skip(d) || walk_gating::always_region(d).is_some())
}

/// A parse holds a body fact another file's resolution reads: a Go
/// `MountArg` flows into the callee's parameter, re-keying ROUTEs there.
fn body_cross_dep(p: &FileParse) -> bool {
    p.nav
        .nav_facts
        .values()
        .flatten()
        .any(|f| matches!(f, NavFact::MountArg { .. }))
}

/// One modified file of one commit.
struct Edit {
    commit: String,
    path: String,
    group: String,
    loose_equal: bool,
    strict_equal: bool,
    cross_dep: bool,
    /// The surface parts the edit moved ([`surface_diff`]), empty when equal.
    moved: Vec<String>,
}

/// A positive-control hit: a top-level declaration line added or removed
/// while the loose surface stayed equal.
struct Violation {
    commit: String,
    path: String,
    group: String,
    lines: Vec<String>,
    explanation: &'static str,
}

/// The keyword a top-level (column 0) declaration line starts with, after
/// visibility / modifier words; `None` for any other line.
fn decl_keyword(line: &str) -> Option<(&'static str, String)> {
    if line.starts_with(char::is_whitespace) {
        return None;
    }
    let mut rest = line;
    loop {
        let before = rest;
        for m in [
            "export ",
            "default ",
            "pub(crate) ",
            "pub(super) ",
            "pub ",
            "async ",
            "abstract ",
            "declare ",
            "unsafe ",
            "const ",
        ] {
            if let Some(r) = rest.strip_prefix(m) {
                rest = r;
            }
        }
        if rest == before {
            break;
        }
    }
    const KEYWORDS: [&str; 10] = [
        "def",
        "class",
        "fn",
        "func",
        "function",
        "interface",
        "struct",
        "impl",
        "trait",
        "type",
    ];
    for k in KEYWORDS {
        if let Some(r) = rest.strip_prefix(k)
            && r.starts_with([' ', '(', '<', '*'])
        {
            return Some((k, decl_name(k, r)));
        }
    }
    None
}

/// The declared name after the keyword: a Go method's receiver is skipped,
/// an `impl .. for T` names `T`.
fn decl_name(keyword: &str, rest: &str) -> String {
    let mut r = rest.trim_start_matches(['*', ' ']);
    if keyword == "func"
        && r.starts_with('(')
        && let Some(close) = r.find(')')
    {
        r = r[close + 1..].trim_start();
    }
    if keyword == "impl"
        && let Some(i) = r.find(" for ")
    {
        r = &r[i + 5..];
    }
    if keyword == "impl" && r.starts_with('<') {
        let mut depth = 0i32;
        for (i, c) in r.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                r = r[i + 1..].trim_start();
                break;
            }
        }
    }
    r.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
        .collect()
}

/// The added (`+`) and removed (`-`) lines of `git diff -U0`.
fn diff_lines(diff: &str) -> (Vec<String>, Vec<String>) {
    let (mut add, mut del) = (Vec::new(), Vec::new());
    for l in diff.lines() {
        if l.starts_with("+++") || l.starts_with("---") {
            continue;
        }
        if let Some(r) = l.strip_prefix('+') {
            add.push(r.to_string());
        } else if let Some(r) = l.strip_prefix('-') {
            del.push(r.to_string());
        }
    }
    (add, del)
}

/// Why a top-level declaration line changed while the loose surface held,
/// or `unexplained`:
/// - `moved`: the same declaration lines were removed and re-added (a move
///   or a reorder; a declaration's row is no surface).
/// - `no_node`: the declared name is no node's last qname segment before or
///   after (the parser mints nothing for it, so no resolution can read it).
/// - `same_name`: every changed declaration keeps its name on both sides and
///   the surface the language records for it did not move (parameters,
///   annotations or formatting the parser does not record).
fn explain(
    add: &[(String, String)],
    del: &[(String, String)],
    names: &BTreeSet<String>,
) -> &'static str {
    let a: BTreeSet<&str> = add.iter().map(|(l, _)| l.trim_end()).collect();
    let d: BTreeSet<&str> = del.iter().map(|(l, _)| l.trim_end()).collect();
    if a == d {
        return "moved";
    }
    let changed: Vec<&String> = add
        .iter()
        .filter(|(l, _)| !d.contains(l.trim_end()))
        .chain(del.iter().filter(|(l, _)| !a.contains(l.trim_end())))
        .map(|(_, n)| n)
        .collect();
    if changed.iter().all(|n| n.is_empty() || !names.contains(*n)) {
        return "no_node";
    }
    let an: BTreeSet<&String> = add.iter().map(|(_, n)| n).collect();
    let dn: BTreeSet<&String> = del.iter().map(|(_, n)| n).collect();
    if changed.iter().all(|n| an.contains(n) && dn.contains(n)) {
        return "same_name";
    }
    "unexplained"
}

/// The last qname segment of every node of `p`.
fn node_names(p: &FileParse, into: &mut BTreeSet<String>) {
    for n in &p.nodes {
        if let Some(q) = p.nav.qname_by_id.get(&n.id) {
            let last = q.rsplit("::").next().unwrap_or(q);
            into.insert(last.to_string());
            into.insert(last.rsplit('.').next().unwrap_or(last).to_string());
        }
    }
}

/// Which parts of the loose surface an edit moved (diagnostic only, never a
/// verdict input): `nodes:+<KIND>` / `nodes:-<KIND>` per node added or
/// removed by (kind, qname), and one name per other part that differs.
fn surface_diff(b: &FileParse, a: &FileParse) -> Vec<String> {
    use glia_cache::{Part, surface_class};
    type Key = (u32, String);
    let keys = |p: &FileParse| -> BTreeSet<Key> {
        p.nodes
            .iter()
            .map(|n| {
                (
                    p.nav.kind_by_id.get(&n.id).map_or(0, |k| k.0),
                    p.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default(),
                )
            })
            .collect()
    };
    let (kb, ka) = (keys(b), keys(a));
    let mut out: BTreeSet<String> = BTreeSet::new();
    let kind = |k: u32| glia_code_domain::node_kind::name(glia_core::NodeKindId(k));
    for (k, _) in ka.difference(&kb) {
        out.insert(format!("nodes:+{}", kind(*k)));
    }
    for (k, _) in kb.difference(&ka) {
        out.insert(format!("nodes:-{}", kind(*k)));
    }
    fn sorted<T: Ord>(v: impl Iterator<Item = T>) -> Vec<T> {
        let mut v: Vec<T> = v.collect();
        v.sort();
        v
    }
    let qn = |p: &FileParse, id: &glia_core::NodeId| p.nav.qname_by_id.get(id).cloned();
    let imports = |p: &FileParse| {
        sorted(
            p.imports
                .iter()
                .map(|i| format!("{:?}{:?}", i.from_module, i.target)),
        )
    };
    if imports(b) != imports(a) {
        out.insert("imports".into());
    }
    let heritage = |p: &FileParse| {
        sorted(
            p.refs
                .iter()
                .filter(|r| {
                    r.category == glia_code_domain::edge_category::INHERITS_FROM
                        || r.category == glia_code_domain::edge_category::IMPLEMENTS
                })
                .map(|r| format!("{:?}{:?}{:?}", qn(p, &r.from), r.qualifier, r.category)),
        )
    };
    if heritage(b) != heritage(a) {
        out.insert("heritage".into());
    }
    let by_q = |p: &FileParse, m: &std::collections::HashMap<glia_core::NodeId, String>| {
        sorted(m.iter().map(|(k, v)| (qn(p, k), v.clone())))
    };
    if by_q(b, &b.nav.return_types) != by_q(a, &a.nav.return_types) {
        out.insert("return_types".into());
    }
    if by_q(b, &b.nav.method_sigs) != by_q(a, &a.nav.method_sigs) {
        out.insert("method_sigs".into());
    }
    let fields = |p: &FileParse| {
        sorted(
            p.nav
                .field_types
                .iter()
                .flat_map(|(k, m)| m.iter().map(move |(f, t)| (qn(p, k), f.clone(), t.clone()))),
        )
    };
    if fields(b) != fields(a) {
        out.insert("field_types".into());
    }
    let locals = |p: &FileParse| {
        sorted(
            p.nav
                .local_types
                .iter()
                .filter(|(k, _)| {
                    p.nav.kind_by_id.get(k) == Some(&glia_code_domain::node_kind::MODULE)
                })
                .flat_map(|(k, m)| m.iter().map(move |(f, t)| (qn(p, k), f.clone(), t.clone()))),
        )
    };
    if locals(b) != locals(a) {
        out.insert("module_locals".into());
    }
    let facts = |p: &FileParse| {
        sorted(p.nav.nav_facts.iter().flat_map(|(k, v)| {
            v.iter()
                .filter(|f| surface_class(f) == Part::Surface)
                .map(move |f| format!("{:?}{f:?}", qn(p, k)))
        }))
    };
    if facts(b) != facts(a) {
        out.insert("facts".into());
    }
    let props = |p: &FileParse| sorted(p.properties.iter().map(|id| qn(p, id)));
    if props(b) != props(a) {
        out.insert("properties".into());
    }
    let tree = |p: &FileParse| {
        sorted(
            p.nav
                .parent_of
                .iter()
                .map(|(c, par)| (qn(p, c), qn(p, par))),
        )
    };
    if tree(b) != tree(a) {
        out.insert("nav_tree".into());
    }
    if out.is_empty() {
        // Only rows moved (line-zeroed facts aside) or an id reordered.
        out.insert("other".into());
    }
    out.into_iter().collect()
}

/// An order-independent pick key for the null control's files.
fn pick_key(commit: &str, path: &str) -> u64 {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};
    BuildHasherDefault::<DefaultHasher>::default().hash_one((commit, path))
}

struct Replay {
    edits: Vec<Edit>,
    violations: Vec<Violation>,
    null_checked: usize,
    null_failures: Vec<String>,
    skipped: BTreeMap<&'static str, usize>,
    commits: usize,
    /// (group, surface part) -> edits whose loose surface it moved.
    why: BTreeMap<(String, String), usize>,
}

fn replay(a: &Args) -> Result<Replay, String> {
    let list = git_text(
        &a.repo,
        &[
            "rev-list",
            "--first-parent",
            "-n",
            &a.commits.to_string(),
            &a.rev,
        ],
    )?;
    let commits: Vec<&str> = list.lines().collect();
    let mut r = Replay {
        edits: Vec::new(),
        violations: Vec::new(),
        null_checked: 0,
        null_failures: Vec::new(),
        skipped: BTreeMap::new(),
        commits: commits.len(),
        why: BTreeMap::new(),
    };
    for (i, c) in commits.iter().enumerate() {
        let parent = format!("{c}^");
        if git(&a.repo, &["rev-parse", "--verify", "-q", &parent]).is_err() {
            *r.skipped.entry("root_commit").or_default() += 1;
            continue;
        }
        let status = git_text(&a.repo, &["diff", "--name-status", "-M", &parent, c])?;
        let mut touched: BTreeSet<String> = BTreeSet::new();
        for line in status.lines() {
            let mut cols = line.split('\t');
            let (Some(st), Some(path)) = (cols.next(), cols.next()) else {
                continue;
            };
            touched.insert(path.to_string());
            for p in cols {
                touched.insert(p.to_string());
            }
            if st != "M" {
                continue;
            }
            let Some(lang) = surface_language(path).filter(|l| is_code(l)) else {
                continue;
            };
            if !walked(path) {
                *r.skipped.entry("outside_walk").or_default() += 1;
                continue;
            }
            let (Ok(before), Ok(after)) = (
                git_text(&a.repo, &["cat-file", "-p", &format!("{parent}:{path}")]),
                git_text(&a.repo, &["cat-file", "-p", &format!("{c}:{path}")]),
            ) else {
                *r.skipped.entry("not_utf8").or_default() += 1;
                continue;
            };
            let (Some(pb), Some(pa)) = (parse(&before, path, lang), parse(&after, path, lang))
            else {
                *r.skipped.entry("parse_error").or_default() += 1;
                continue;
            };
            let loose_equal = matches!(
                (surface_fingerprint(&pb), surface_fingerprint(&pa)),
                (Some(x), Some(y)) if x == y
            );
            let strict_equal = matches!(
                (surface_fingerprint_strict(&pb), surface_fingerprint_strict(&pa)),
                (Some(x), Some(y)) if x == y
            );
            let cross_dep = body_cross_dep(&pb) || body_cross_dep(&pa);
            let group = group_of(lang).to_string();
            let moved = if loose_equal {
                Vec::new()
            } else {
                surface_diff(&pb, &pa)
            };
            for part in &moved {
                *r.why.entry((group.clone(), part.clone())).or_default() += 1;
            }
            if loose_equal {
                let diff = git_text(&a.repo, &["diff", "-U0", &parent, c, "--", path])?;
                let (add, del) = diff_lines(&diff);
                let keep = |v: Vec<String>| -> Vec<(String, String)> {
                    v.into_iter()
                        .filter_map(|l| decl_keyword(&l).map(|(_, n)| (l, n)))
                        .collect()
                };
                let (add, del) = (keep(add), keep(del));
                if !add.is_empty() || !del.is_empty() {
                    let mut names = BTreeSet::new();
                    node_names(&pb, &mut names);
                    node_names(&pa, &mut names);
                    let explanation = explain(&add, &del, &names);
                    let lines = del
                        .iter()
                        .map(|(l, _)| format!("-{l}"))
                        .chain(add.iter().map(|(l, _)| format!("+{l}")))
                        .collect();
                    r.violations.push(Violation {
                        commit: c.to_string(),
                        path: path.to_string(),
                        group: group.clone(),
                        lines,
                        explanation,
                    });
                }
            }
            r.edits.push(Edit {
                commit: c.to_string(),
                path: path.to_string(),
                group,
                loose_equal,
                strict_equal,
                cross_dep,
                moved,
            });
        }
        // The null control: unmodified source files, parsed twice.
        let tree = git_text(&a.repo, &["ls-tree", "-r", "--name-only", c])?;
        let mut pool: Vec<&str> = tree
            .lines()
            .filter(|p| !touched.contains(*p) && walked(p))
            .filter(|p| surface_language(p).is_some_and(is_code))
            .collect();
        pool.sort_by_key(|p| (pick_key(c, p), *p));
        for path in pool.into_iter().take(NULL_PER_COMMIT) {
            let Some(lang) = surface_language(path) else {
                continue;
            };
            let Ok(src) = git_text(&a.repo, &["cat-file", "-p", &format!("{c}:{path}")]) else {
                continue;
            };
            let (Some(p1), Some(p2)) = (parse(&src, path, lang), parse(&src, path, lang)) else {
                continue;
            };
            r.null_checked += 1;
            if surface_fingerprint(&p1) != surface_fingerprint(&p2)
                || surface_fingerprint_strict(&p1) != surface_fingerprint_strict(&p2)
                || surface_fingerprint(&p1).is_none()
            {
                r.null_failures.push(format!("{c} {path}"));
            }
        }
        if (i + 1) % 25 == 0 {
            eprintln!(
                "[surface-replay] progress repo={} commits={}/{} edits={}",
                a.label,
                i + 1,
                commits.len(),
                r.edits.len()
            );
        }
    }
    Ok(r)
}

/// The comment line inserted for a body-only touch of `path`.
fn comment_for(path: &str) -> Option<&'static str> {
    let ext = Path::new(path).extension()?.to_str()?;
    match ext {
        "py" | "rb" | "ex" | "exs" | "tf" | "hcl" => Some("# glia surface-replay touch\n"),
        "lua" => Some("-- glia surface-replay touch\n"),
        "go" | "rs" | "ts" | "tsx" | "js" | "jsx" | "java" | "kt" | "cs" | "swift" | "c" | "cc"
        | "cpp" | "h" | "hpp" | "scala" | "dart" | "php" | "sol" => {
            Some("// glia surface-replay touch\n")
        }
        _ => None,
    }
}

/// The median of `v` (0 when empty).
fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn load_avg() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(String::from))
        .unwrap_or_else(|| "?".into())
}

/// Median phase_ms of one group's body-only updates.
struct Timing {
    phases: BTreeMap<&'static str, f64>,
    updates: usize,
    file: String,
    load: String,
}

fn phase_fields(p: &WatchPhases) -> [(&'static str, f64); 14] {
    [
        ("read", p.read),
        ("walk", p.walk),
        ("parse", p.parse),
        ("const_scan", p.const_scan),
        ("grafts", p.grafts),
        ("language_build", p.language_build),
        ("fingerprint", p.fingerprint),
        ("clone", p.clone),
        ("resolve", p.resolve),
        ("post", p.post),
        ("finalize", p.finalize),
        ("external_cells", p.external_cells),
        ("delta", p.delta),
        ("total", p.total),
    ]
}

/// Time `UPDATES` body-only updates of each group in `groups` on a
/// `git archive` copy of `rev`; `files` are the group's edited paths, most
/// edited first, the touched file being the first that exists in the copy.
fn time_groups(
    a: &Args,
    groups: &BTreeMap<String, Vec<String>>,
) -> Result<BTreeMap<String, Timing>, String> {
    let work = a
        .work
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join(format!("surface-replay-{}", a.label)));
    let tree = work.join(&a.label);
    if tree.exists() {
        std::fs::remove_dir_all(&tree).map_err(|e| format!("{}: {e}", tree.display()))?;
    }
    std::fs::create_dir_all(&tree).map_err(|e| format!("{}: {e}", tree.display()))?;
    let tar = git(&a.repo, &["archive", "--format=tar", &a.rev])?;
    let tar_path = work.join(format!("{}.tar", a.label));
    std::fs::write(&tar_path, tar).map_err(|e| format!("{}: {e}", tar_path.display()))?;
    let st = Command::new("tar")
        .arg("-xf")
        .arg(&tar_path)
        .arg("-C")
        .arg(&tree)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    let _ = std::fs::remove_file(&tar_path);
    if !st.success() {
        return Err("tar failed".into());
    }
    let root = tree.to_string_lossy().to_string();
    let opts = glia_engine::BuildOptions::default();
    let mut session = WatchSession::open(&root, &opts)?;
    let mut out = BTreeMap::new();
    for (group, files) in groups {
        let Some(file) = files
            .iter()
            .filter(|f| comment_for(f).is_some() && !f.ends_with(".vue"))
            .find(|f| tree.join(f).is_file())
        else {
            eprintln!(
                "[surface-replay] no touchable file repo={} lang={group}",
                a.label
            );
            continue;
        };
        let abs = tree.join(file);
        let original =
            std::fs::read_to_string(&abs).map_err(|e| format!("{}: {e}", abs.display()))?;
        let touched = format!("{}{original}", comment_for(file).unwrap_or("//\n"));
        let mut kept: Vec<WatchUpdate> = Vec::new();
        let mut load = String::new();
        let mut on = false;
        for _ in 0..UPDATES * 3 {
            if kept.len() == UPDATES {
                break;
            }
            on = !on;
            let text = if on { &touched } else { &original };
            std::fs::write(&abs, text).map_err(|e| format!("{}: {e}", abs.display()))?;
            load = load_avg();
            let u = session.apply(std::slice::from_ref(&abs))?;
            if u.groups_rebuilt.len() == 1
                && u.groups_rebuilt[0] == *group
                && u.full_rebuild.is_none()
            {
                kept.push(u);
            } else {
                eprintln!(
                    "[surface-replay] update dropped repo={} lang={group} rebuilt={:?} full={:?}",
                    a.label, u.groups_rebuilt, u.full_rebuild
                );
            }
        }
        if on {
            std::fs::write(&abs, &original).map_err(|e| format!("{}: {e}", abs.display()))?;
            session.apply(std::slice::from_ref(&abs))?;
        }
        let mut phases = BTreeMap::new();
        for (name, _) in phase_fields(&WatchPhases::default()) {
            let v: Vec<f64> = kept
                .iter()
                .map(|u| {
                    phase_fields(&u.phase_ms)
                        .iter()
                        .find(|(n, _)| *n == name)
                        .map(|(_, v)| *v)
                        .unwrap_or(0.0)
                })
                .collect();
            phases.insert(name, median(v));
        }
        out.insert(
            group.clone(),
            Timing {
                phases,
                updates: kept.len(),
                file: file.clone(),
                load,
            },
        );
    }
    drop(session);
    let _ = std::fs::remove_dir_all(&tree);
    Ok(out)
}

fn threads_label() -> String {
    match std::env::var("GLIA_THREADS") {
        Ok(v) if !v.is_empty() && v != "0" => v,
        _ => format!(
            "default({})",
            std::thread::available_parallelism().map_or(1, |n| n.get())
        ),
    }
}

fn round(x: f64, places: i32) -> f64 {
    let m = 10f64.powi(places);
    (x * m).round() / m
}

fn run() -> Result<i32, String> {
    let a = args()?;
    let r = replay(&a)?;
    let threads = threads_label();
    let null_ok = r.null_failures.is_empty();
    let unexplained = r
        .violations
        .iter()
        .filter(|v| v.explanation == "unexplained")
        .count();
    eprintln!(
        "[surface-replay] controls repo={} null_checked={} null_failures={} violations={} unexplained={unexplained}",
        a.label,
        r.null_checked,
        r.null_failures.len(),
        r.violations.len()
    );
    for f in &r.null_failures {
        eprintln!("[surface-replay] null-control failure {f}");
    }
    if !null_ok {
        eprintln!(
            "[surface-replay] FAILED repo={}: the null control failed, no verdict",
            a.label
        );
        return Ok(1);
    }
    let controls_ok = unexplained == 0;

    // Per group: the edits and the files most edited first.
    let mut by_group: BTreeMap<String, Vec<&Edit>> = BTreeMap::new();
    for e in &r.edits {
        by_group.entry(e.group.clone()).or_default().push(e);
    }
    let mut touch: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (g, edits) in &by_group {
        let mut count: BTreeMap<&str, usize> = BTreeMap::new();
        for e in edits {
            *count.entry(e.path.as_str()).or_default() += 1;
        }
        let mut files: Vec<(&str, usize)> = count.into_iter().collect();
        files.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(y.0)));
        touch.insert(
            g.clone(),
            files.into_iter().map(|(p, _)| p.to_string()).collect(),
        );
    }
    let timings = if a.watch {
        time_groups(&a, &touch)?
    } else {
        BTreeMap::new()
    };

    let mut rows: Vec<Value> = Vec::new();
    for (g, edits) in &by_group {
        let n = edits.len();
        let loose = edits
            .iter()
            .filter(|e| e.loose_equal && !e.cross_dep)
            .count();
        let strict = edits
            .iter()
            .filter(|e| e.strict_equal && !e.cross_dep)
            .count();
        let cross = edits.iter().filter(|e| e.cross_dep).count();
        let share = |k: usize| if n == 0 { 0.0 } else { k as f64 / n as f64 };
        let (bl, bs) = (share(loose), share(strict));
        let t = timings.get(g);
        let ph = |k: &str| t.and_then(|t| t.phases.get(k).copied()).unwrap_or(0.0);
        let group_ms = ph("language_build");
        let update_ms = ph("total");
        let clone_ms = ph("clone");
        let build_share = if update_ms > 0.0 {
            group_ms / update_ms
        } else {
            0.0
        };
        let saving = bl * group_ms;
        let group_controls = controls_ok
            && !r
                .violations
                .iter()
                .any(|v| v.group == *g && v.explanation == "unexplained");
        let pass = t.is_some_and(|t| t.updates > 0)
            && n >= MIN_EDITS
            && bl >= MIN_BODY_ONLY
            && group_ms >= MIN_GROUP_MS
            && build_share >= MIN_BUILD_SHARE
            && group_controls;
        let clone_pass = clone_ms >= 50.0 && clone_ms >= 0.20 * update_ms;
        let load = t.map_or("-".to_string(), |t| t.load.clone());
        println!(
            "[surface-replay] repo={} lang={g} edits={n} body_only={:.3}/{:.3} group_ms={:.1} update_ms={:.1} build_share={:.3} clone_ms={:.1} saving_ms={:.1} controls={} verdict={} load={load} threads={threads}",
            a.label,
            bl,
            bs,
            group_ms,
            update_ms,
            build_share,
            clone_ms,
            saving,
            if group_controls { "ok" } else { "fail" },
            if pass { "PASS" } else { "FAIL" },
        );
        let tail = ph("resolve") + ph("post") + ph("finalize");
        println!(
            "[surface-replay] decomp repo={} lang={g} group_ms={:.1} grafts_ms={:.1} tail_ms={:.1} delta_ms={:.1} clone_ms={:.1} update_ms={:.1} threads={threads}",
            a.label,
            group_ms,
            ph("grafts"),
            tail,
            ph("delta"),
            clone_ms,
            update_ms,
        );
        let phases: BTreeMap<&str, f64> = t
            .map(|t| t.phases.iter().map(|(k, v)| (*k, round(*v, 1))).collect())
            .unwrap_or_default();
        rows.push(json!({
            "repo": a.label,
            "lang": g,
            "edits": n,
            "body_only_loose": round(bl, 4),
            "body_only_strict": round(bs, 4),
            "cross_dep_edits": cross,
            "loose_equal": edits.iter().filter(|e| e.loose_equal).count(),
            "strict_equal": edits.iter().filter(|e| e.strict_equal).count(),
            "group_ms": round(group_ms, 1),
            "update_ms": round(update_ms, 1),
            "build_share": round(build_share, 4),
            "clone_ms": round(clone_ms, 1),
            "saving_ms": round(saving, 1),
            "projected_update_ms": round(update_ms - saving, 1),
            "phase_ms": phases,
            "timed_updates": t.map_or(0, |t| t.updates),
            "touched_file": t.map(|t| t.file.clone()),
            "load_avg": load,
            "threads": threads,
            "controls": if group_controls { "ok" } else { "fail" },
            "verdict": if pass { "PASS" } else { "FAIL" },
            "clone_rule": if clone_pass { "PASS" } else { "FAIL" },
        }));
    }
    let violations: Vec<Value> = r
        .violations
        .iter()
        .map(|v| {
            json!({
                "commit": v.commit,
                "path": v.path,
                "lang": v.group,
                "explanation": v.explanation,
                "lines": v.lines,
            })
        })
        .collect();
    let mut by_expl: BTreeMap<&str, usize> = BTreeMap::new();
    for v in &r.violations {
        *by_expl.entry(v.explanation).or_default() += 1;
    }
    let edits: Vec<Value> = r
        .edits
        .iter()
        .map(|e| {
            json!([
                e.commit,
                e.path,
                e.group,
                e.loose_equal,
                e.strict_equal,
                e.cross_dep,
                e.moved
            ])
        })
        .collect();
    let doc = json!({
        "repo": a.label,
        "rev": git_text(&a.repo, &["rev-parse", &a.rev])?.trim(),
        "commits": r.commits,
        "threads": threads,
        "rows": rows,
        "controls": {
            "null_checked": r.null_checked,
            "null_failures": r.null_failures,
            "positive_violations": r.violations.len(),
            "by_explanation": by_expl,
        },
        "skipped": r.skipped,
        "surface_moved_by": r
            .why
            .iter()
            .map(|((g, part), n)| json!([g, part, n]))
            .collect::<Vec<Value>>(),
        "violations": violations,
        "edits": edits,
    });
    if let Some(out) = &a.out {
        let text = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        std::fs::write(out, text).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    Ok(0)
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("[surface-replay] error: {e}");
            std::process::exit(2);
        }
    }
}
