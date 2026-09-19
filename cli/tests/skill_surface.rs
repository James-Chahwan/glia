//! LG.15 — `skills/glia/SKILL.md`, the Claude Code skill for using glia
//! through its CLI without the MCP server, stays true to the CLI.
//!
//! Every `glia ...` invocation in the skill (a line of a fenced code block, or
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
//! stale mention. The skill must also carry a `--json` example of each command
//! the packet names, which keeps the check from passing vacuously on a skill
//! the parser reads no invocation from.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const SKILL_REL: &str = "skills/glia/SKILL.md";

/// The commands LG.15 gives a worked `--json` example each.
const EXAMPLED: [&str; 18] = [
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

/// Check one invocation's words against the surface; a finding per problem.
fn check(surface: &Surface, args: &[String]) -> Vec<String> {
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
                    return found;
                }
            } else if wants_sub && !a.starts_with('<') {
                match surface.subcommands[&path[0]].get(a) {
                    Some(sub) => path.push(sub.clone()),
                    None => {
                        found.push(format!("unknown subcommand `glia {} {a}`", path[0]));
                        return found;
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
    found
}

/// Every stale mention in `md`, as `<rel>:<line>: ...`.
fn stale_mentions(surface: &Surface, md: &str, rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (line, args) in invocations(md) {
        for finding in check(surface, &args) {
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

fn skill_text() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(SKILL_REL);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn skill_invocations_match_the_cli_surface() {
    let surface = surface();
    let md = skill_text();
    let stale = stale_mentions(&surface, &md, SKILL_REL);
    assert!(
        stale.is_empty(),
        "{} stale glia mention(s) in {SKILL_REL} (check against cli/surface/*.txt):\n{}",
        stale.len(),
        stale.join("\n")
    );

    let calls = invocations(&md);
    let missing: Vec<&str> = EXAMPLED
        .iter()
        .copied()
        .filter(|cmd| {
            !calls.iter().any(|(_, a)| {
                a.iter()
                    .find(|w| !w.starts_with('-'))
                    .is_some_and(|w| w == cmd)
                    && a.iter().any(|w| w == "--json")
            })
        })
        .collect();
    assert!(
        missing.is_empty(),
        "{SKILL_REL} has no `glia <cmd> ... --json` example of: {}",
        missing.join(", ")
    );
}

#[test]
fn skill_frontmatter_names_the_skill() {
    let md = skill_text();
    let front = md
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map(|(f, _)| f)
        .unwrap_or_else(|| panic!("{SKILL_REL} opens with no `---` frontmatter block"));
    assert!(
        front.lines().any(|l| l == "name: glia"),
        "frontmatter lacks `name: glia`"
    );
    assert!(
        front
            .lines()
            .any(|l| l.starts_with("description: ") && l.len() > 60),
        "frontmatter lacks a `description:` saying when to use the skill"
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
