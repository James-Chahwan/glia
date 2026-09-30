//! LG.15 / CE.3f — every Claude Code skill under `skills/` (`skills/<name>/SKILL.md`:
//! `glia`, using glia through its CLI without the MCP server, and `glia-overlay`,
//! the overlay loop's model step) stays true to the CLI.
//!
//! Every `glia ...` invocation in a skill (a line of a fenced code block, or
//! an inline code span, whose first word after any `VAR=value` prefix is
//! `glia`; `|`, `&&`, `||` and `;` start a new command) is checked against the
//! committed CLI surface snapshots in `cli/surface/` (LG.6a):
//! - its command has a `cli/surface/<command>.txt`, and a subcommand is one
//!   that snapshot declares (`cmd glia cell ls`);
//! - every `--flag` / `-s` it passes is declared by that command, its
//!   subcommand or `_global.txt`;
//! - a literal value given to a flag with a closed value set
//!   (`--direction forward`) is one of the snapshot's `values=`.
//!
//! A `<placeholder>` in the command position (`glia <command> --help`) names no
//! snapshot: its flags must be global. A failure lists `file:line` for each
//! stale mention. Each skill must also carry a `--json` example of every
//! command its [`EXAMPLED`] entry names (a subcommand as `overlay propose`,
//! which must be a `cmd glia overlay propose` of the surface), which keeps the
//! check from passing vacuously on a skill the parser reads no invocation
//! from; a skill directory with no entry fails until one is added.
//!
//! fired_on: one stderr line per skill, in name order,
//! `[skill] <name>: <n> invocations checked against cli/surface (<k> stale)`
//! (`cargo test -p glia-cli --test skill_surface -- --nocapture`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The skills directory, relative to the repo root.
const SKILLS_REL: &str = "skills";

/// Per skill directory, the commands its worked examples show with `--json`:
/// `glia` (LG.15) and `glia-overlay` (CE.3f). A subcommand is its surface
/// section path, `overlay propose`.
const EXAMPLED: [(&str, &[&str]); 2] = [
    (
        "glia",
        &[
            "arch",
            "find",
            "resolve",
            "blast-radius",
            "diff-impact",
            "trace",
            "flows",
            "why",
            "serves",
            "implementors",
            "effects",
            "tests-for",
            "delta",
            "cycles",
            "check",
            "coverage",
            "gaps",
            "docs-for",
        ],
    ),
    (
        "glia-overlay",
        &[
            "gaps",
            "overlay propose",
            "overlay try",
            "overlay accept",
            "find",
        ],
    ),
];

struct ArgSpec {
    takes_value: bool,
    values: Vec<String>,
}

/// The CLI surface as the snapshots pin it. Section keys are the command path
/// below `glia`, space-joined: `""` for the global options, `"find"`,
/// `"cell ls"`.
#[derive(Default)]
struct Surface {
    flags: BTreeMap<String, BTreeMap<String, ArgSpec>>,
    /// Command name or alias -> canonical command name.
    commands: BTreeMap<String, String>,
    /// Canonical command -> (subcommand name or alias -> canonical name).
    subcommands: BTreeMap<String, BTreeMap<String, String>>,
}

impl Surface {
    fn load(dir: &Path) -> Surface {
        let mut s = Surface::default();
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "txt"))
            .collect();
        files.sort();
        for f in &files {
            let text = std::fs::read_to_string(f).expect("read a surface snapshot");
            s.add_snapshot(&text);
        }
        assert!(
            s.flags.contains_key(""),
            "no _global.txt in {}",
            dir.display()
        );
        s
    }

    fn add_snapshot(&mut self, text: &str) {
        let mut section = String::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("cmd glia") {
                let mut path: Vec<&str> = Vec::new();
                let mut aliases: Vec<&str> = Vec::new();
                for word in rest.split_whitespace() {
                    if let Some(list) = word.strip_prefix("aliases=") {
                        aliases = list.split(',').collect();
                    } else if word != "hidden" && word != "subcommand_required" {
                        path.push(word);
                    }
                }
                section = path.join(" ");
                self.flags.entry(section.clone()).or_default();
                match path.as_slice() {
                    [cmd] => {
                        for name in std::iter::once(*cmd).chain(aliases) {
                            self.commands.insert(name.to_string(), cmd.to_string());
                        }
                    }
                    [cmd, sub] => {
                        let subs = self.subcommands.entry(cmd.to_string()).or_default();
                        for name in std::iter::once(*sub).chain(aliases) {
                            subs.insert(name.to_string(), sub.to_string());
                        }
                    }
                    _ => {}
                }
            } else if let Some(rest) = line.trim_start().strip_prefix("arg ") {
                let kv: BTreeMap<&str, &str> = rest
                    .split_whitespace()
                    .filter_map(|w| w.split_once('='))
                    .collect();
                let spec = || ArgSpec {
                    takes_value: kv.get("num_args").is_some_and(|n| *n != "0"),
                    values: kv
                        .get("values")
                        .map(|v| v.split(',').map(str::to_string).collect())
                        .unwrap_or_default(),
                };
                let mut names: Vec<String> = Vec::new();
                if let Some(l) = kv.get("long").filter(|l| **l != "-") {
                    names.push(format!("--{l}"));
                }
                if let Some(c) = kv.get("short").filter(|c| **c != "-") {
                    names.push(format!("-{c}"));
                }
                if let Some(list) = kv.get("aliases") {
                    names.extend(list.split(',').map(|a| format!("--{a}")));
                }
                let flags = self.flags.entry(section.clone()).or_default();
                for n in names {
                    flags.insert(n, spec());
                }
            }
        }
    }

    /// The flag's spec in the innermost of `path`'s sections that declares
    /// it, else the global options.
    fn flag(&self, path: &[String], name: &str) -> Option<&ArgSpec> {
        (0..=path.len())
            .rev()
            .find_map(|n| self.flags.get(&path[..n].join(" "))?.get(name))
    }
}

/// Split one shell-ish line into words, honouring '…' and "…" quoting; an
/// unquoted `#` opening a word starts a comment that runs to the line's end.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '#' && !in_word => break,
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    out.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            None => {
                cur.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        out.push(cur);
    }
    out
}

/// `(1-based line, text)` of every code-block line (`\` continuations
/// joined) and every inline code span.
fn code_texts(md: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut pending: Option<(usize, String)> = None;
    for (i, raw) in md.lines().enumerate() {
        let t = raw.trim();
        if t.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            let (start, mut text) = pending.take().unwrap_or((i + 1, String::new()));
            if let Some(head) = t.strip_suffix('\\') {
                text.push_str(head);
                text.push(' ');
                pending = Some((start, text));
                continue;
            }
            text.push_str(t);
            out.push((start, text));
        } else {
            out.extend(
                raw.split('`')
                    .skip(1)
                    .step_by(2)
                    .map(|span| (i + 1, span.to_string())),
            );
        }
    }
    out
}

/// Every `glia` invocation in `md`: its line and the words after `glia`.
fn invocations(md: &str) -> Vec<(usize, Vec<String>)> {
    let mut out = Vec::new();
    for (line, text) in code_texts(md) {
        let all = words(&text);
        for segment in all.split(|w| matches!(w.as_str(), "|" | "&&" | "||" | ";")) {
            let mut rest = segment.iter().skip_while(|w| {
                !w.starts_with('-')
                    && w.split_once('=').is_some_and(|(k, _)| {
                        !k.is_empty() && k.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                    })
            });
            if rest.next().is_some_and(|w| w == "glia") {
                out.push((line, rest.cloned().collect()));
            }
        }
    }
    out
}

/// Check one invocation's words against the surface: the command path it
/// resolves to (canonical names, `["overlay", "try"]`; it stops at a
/// placeholder or an unknown word, so `glia <command>` resolves to `[]`) and a
/// finding per problem.
fn check(surface: &Surface, args: &[String]) -> (Vec<String>, Vec<String>) {
    let mut found = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut placeholder = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if a == "--" {
            break;
        }
        let is_flag = a.starts_with("--") || (a.starts_with('-') && a.len() == 2 && a != "-");
        if !is_flag {
            let wants_sub = path.len() == 1 && surface.subcommands.contains_key(&path[0]);
            if path.is_empty() && !placeholder {
                if a.starts_with('<') {
                    placeholder = true;
                } else if let Some(cmd) = surface.commands.get(a) {
                    path.push(cmd.clone());
                } else {
                    found.push(format!("unknown command `{a}` (no cli/surface/{a}.txt)"));
                    return (path, found);
                }
            } else if wants_sub && !a.starts_with('<') {
                match surface.subcommands[&path[0]].get(a) {
                    Some(sub) => path.push(sub.clone()),
                    None => {
                        found.push(format!("unknown subcommand `glia {} {a}`", path[0]));
                        return (path, found);
                    }
                }
            }
            continue;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (a.as_str(), None),
        };
        let Some(spec) = surface.flag(&path, name) else {
            let owner = if path.is_empty() {
                "the global options".to_string()
            } else {
                format!("`glia {}`", path.join(" "))
            };
            found.push(format!("unknown flag `{name}` for {owner}"));
            continue;
        };
        if !spec.takes_value {
            continue;
        }
        let value = inline.or_else(|| {
            let v = args.get(i).cloned();
            i += 1;
            v
        });
        if let Some(v) = value
            && !spec.values.is_empty()
            && !v.starts_with('<')
            && !spec.values.contains(&v)
        {
            found.push(format!(
                "`{name} {v}`: not one of {}",
                spec.values.join(", ")
            ));
        }
    }
    (path, found)
}

/// Every stale mention in `md`, as `<rel>:<line>: ...`.
fn stale_mentions(surface: &Surface, md: &str, rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (line, args) in invocations(md) {
        for finding in check(surface, &args).1 {
            out.push(format!(
                "{rel}:{line}: `glia {}`: {finding}",
                args.join(" ")
            ));
        }
    }
    out
}

fn surface() -> Surface {
    Surface::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("surface"))
}

fn skills_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(SKILLS_REL)
}

/// Every `<dir>/<name>/`, in name order, with its `SKILL.md` path.
fn skill_dirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .map(|p| {
            let name = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_else(|| panic!("non-UTF-8 skill directory {}", p.display()))
                .to_string();
            (name, p.join("SKILL.md"))
        })
        .collect();
    out.sort();
    out
}

/// One skill's check.
struct SkillReport {
    name: String,
    /// `skills/<name>/SKILL.md`, what each finding is prefixed with.
    rel: String,
    invocations: usize,
    /// `<rel>:<line>: ...` per stale mention.
    stale: Vec<String>,
    /// Commands its [`EXAMPLED`] entry names with no `--json` example.
    missing: Vec<String>,
}

impl SkillReport {
    /// The fired_on line.
    fn marker(&self) -> String {
        format!(
            "[skill] {}: {} invocations checked against cli/surface ({} stale)",
            self.name,
            self.invocations,
            self.stale.len()
        )
    }
}

/// Check every skill under `dir` against the surface, each against its
/// entry of `exampled`. Returns a report per skill, in name order, and the
/// problems no single mention carries: a directory with no `SKILL.md` or no
/// `exampled` entry, an entry naming no skill or no surface command.
fn check_skills(
    surface: &Surface,
    dir: &Path,
    exampled: &[(&str, &[&str])],
) -> (Vec<SkillReport>, Vec<String>) {
    let mut reports = Vec::new();
    let mut problems = Vec::new();
    let dirs = skill_dirs(dir);
    for (skill, _) in exampled {
        if !dirs.iter().any(|(name, _)| name == skill) {
            problems.push(format!(
                "EXAMPLED names skill `{skill}`, but there is no {SKILLS_REL}/{skill}/SKILL.md"
            ));
        }
    }
    for (name, path) in dirs {
        let rel = format!("{SKILLS_REL}/{name}/SKILL.md");
        let Ok(md) = std::fs::read_to_string(&path) else {
            problems.push(format!("{SKILLS_REL}/{name}/ holds no readable SKILL.md"));
            continue;
        };
        let Some(&(_, wanted)) = exampled.iter().find(|(skill, _)| *skill == name) else {
            problems.push(format!(
                "{rel}: no EXAMPLED entry in cli/tests/skill_surface.rs; add its worked-example list"
            ));
            continue;
        };
        let calls = invocations(&md);
        let paths: Vec<(String, bool)> = calls
            .iter()
            .map(|(_, args)| {
                (
                    check(surface, args).0.join(" "),
                    args.iter().any(|w| w == "--json"),
                )
            })
            .collect();
        let mut missing = Vec::new();
        for cmd in wanted.iter().copied() {
            if !surface.flags.contains_key(cmd) {
                problems.push(format!(
                    "{rel}: EXAMPLED names `glia {cmd}`, which has no `cmd glia {cmd}` in cli/surface"
                ));
            } else if !paths.iter().any(|(p, json)| p == cmd && *json) {
                missing.push(cmd.to_string());
            }
        }
        reports.push(SkillReport {
            stale: stale_mentions(surface, &md, &rel),
            invocations: calls.len(),
            name,
            rel,
            missing,
        });
    }
    (reports, problems)
}

#[test]
fn skill_invocations_match_the_cli_surface() {
    let surface = surface();
    let (reports, problems) = check_skills(&surface, &skills_dir(), &EXAMPLED);
    let mut failures = problems;
    for r in &reports {
        eprintln!("{}", r.marker());
        if !r.stale.is_empty() {
            failures.push(format!(
                "{} stale glia mention(s) in {} (check against cli/surface/*.txt):\n{}",
                r.stale.len(),
                r.rel,
                r.stale.join("\n")
            ));
        }
        if !r.missing.is_empty() {
            failures.push(format!(
                "{} has no `glia <cmd> ... --json` example of: {}",
                r.rel,
                r.missing.join(", ")
            ));
        }
    }
    assert!(
        reports.iter().any(|r| r.name == "glia")
            && reports.iter().any(|r| r.name == "glia-overlay"),
        "the glia and glia-overlay skills are both checked"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn skill_frontmatter_names_the_skill() {
    for (name, path) in skill_dirs(&skills_dir()) {
        let md = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let front = md
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(f, _)| f)
            .unwrap_or_else(|| panic!("{name}/SKILL.md opens with no `---` frontmatter block"));
        assert!(
            front.lines().any(|l| l == format!("name: {name}")),
            "{name}/SKILL.md frontmatter lacks `name: {name}`"
        );
        assert!(
            front
                .lines()
                .any(|l| l.starts_with("description: ") && l.len() > 60),
            "{name}/SKILL.md frontmatter lacks a `description:` saying when to use the skill"
        );
    }
}

/// The negative case over a whole skills directory: a broken copy of the
/// glia-overlay skill (its text plus `glia overlay try --candidat x`) is
/// reported at that line, a command the list names with no example is
/// missing, and a skill directory or a listed command the map cannot tie to
/// the surface is a problem.
#[test]
fn a_broken_skills_dir_is_reported() {
    let surface = surface();
    let real = std::fs::read_to_string(skills_dir().join("glia-overlay").join("SKILL.md"))
        .expect("read the glia-overlay skill");
    let broken = format!("{real}\n```bash\nglia overlay try --candidat x\n```\n");
    let bad_line = broken
        .lines()
        .position(|l| l == "glia overlay try --candidat x")
        .expect("the broken line")
        + 1;

    let root = std::env::temp_dir().join(format!("glia-skill-surface-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (name, text) in [
        ("glia-overlay", broken.as_str()),
        (
            "glia-extra",
            "---\nname: glia-extra\n---\n`glia find . X --json`\n",
        ),
    ] {
        let d = root.join(name);
        std::fs::create_dir_all(&d).expect("mkdir");
        std::fs::write(d.join("SKILL.md"), text).expect("write");
    }
    let exampled: [(&str, &[&str]); 1] = [(
        "glia-overlay",
        &[
            "gaps",
            "overlay propose",
            "overlay try",
            "overlay accept",
            "find",
            "spec-status",
            "overlay bogus",
        ],
    )];
    let (reports, problems) = check_skills(&surface, &root, &exampled);
    std::fs::remove_dir_all(&root).ok();

    let names: Vec<&str> = reports.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        ["glia-overlay"],
        "glia-extra has no list: not checked"
    );
    let r = &reports[0];
    assert_eq!(r.stale.len(), 1, "{}", r.stale.join("\n"));
    assert!(
        r.stale[0].starts_with(&format!(
            "skills/glia-overlay/SKILL.md:{bad_line}: `glia overlay try --candidat x`"
        )),
        "{}",
        r.stale[0]
    );
    assert!(
        r.stale[0].contains("unknown flag `--candidat` for `glia overlay try`"),
        "{}",
        r.stale[0]
    );
    assert_eq!(
        r.marker(),
        format!(
            "[skill] glia-overlay: {} invocations checked against cli/surface (1 stale)",
            r.invocations
        )
    );
    assert_eq!(r.missing, ["spec-status"]);
    assert_eq!(problems.len(), 2, "{}", problems.join("\n"));
    assert!(
        problems[0].contains("skills/glia-extra/SKILL.md: no EXAMPLED entry")
            && problems[0].ends_with("add its worked-example list"),
        "{}",
        problems[0]
    );
    assert!(
        problems[1].contains("`glia overlay bogus`, which has no `cmd glia overlay bogus`"),
        "{}",
        problems[1]
    );
}

/// The mutation check, kept: misspelled commands, subcommands, flags and
/// values are each reported at their line; well-formed mentions are not.
#[test]
fn checker_reports_each_misspelling_at_its_line() {
    let surface = surface();
    let md = "\
Use `glia fnd . X` to look.
```bash
glia find . X --jsn
glia blast-radius . X --direction sideways
GLIA_NO_PERSIST=1 glia cell lss .
glia --no-overlay find . X --json --top-k 5
git diff | glia diff-impact . --diff - --json
glia find . X --json   # --bogus sits in a comment
glia trace . checkout \\
  --max-path 3
# glia comment --not-checked
```
Fine: `glia --version`, `glia <command> --help`, `glia cell ls <repo> --json`, `glia effects . X --class=db`.
Bad: `glia <command> --json`.
";
    let got = stale_mentions(&surface, md, "SKILL.md");
    let lines: Vec<&str> = got.iter().filter_map(|f| f.split(": `").next()).collect();
    assert_eq!(
        lines,
        [
            "SKILL.md:1",
            "SKILL.md:3",
            "SKILL.md:4",
            "SKILL.md:5",
            "SKILL.md:9",
            "SKILL.md:14"
        ],
        "findings:\n{}",
        got.join("\n")
    );
    assert!(got[0].contains("unknown command `fnd`"), "{}", got[0]);
    assert!(
        got[1].contains("unknown flag `--jsn` for `glia find`"),
        "{}",
        got[1]
    );
    assert!(
        got[2].contains("`--direction sideways`: not one of forward, backward, both"),
        "{}",
        got[2]
    );
    assert!(
        got[3].contains("unknown subcommand `glia cell lss`"),
        "{}",
        got[3]
    );
    assert!(
        got[4].contains("unknown flag `--max-path` for `glia trace`"),
        "{}",
        got[4]
    );
    assert!(
        got[5].contains("unknown flag `--json` for the global options"),
        "{}",
        got[5]
    );
}
