//! CZ.1 — README.md `## CLI` stays true to the committed CLI surface
//! snapshots in `cli/surface/` (LG.6a). The README block is hand-written, one
//! usage line per (sub)command; this test is what keeps it from drifting.
//!
//! A usage line is a line of a fenced block inside `## CLI` that starts, at
//! column 0, with `glia `. Every usage line is checked:
//! - its command and subcommand exist in the snapshots and are not `hidden`
//!   (`glia timeline build|history|as-of` names three subcommands at once);
//! - every `--flag` / `-x` it shows is declared, not `hidden`, by that command,
//!   its subcommand or `_global.txt`;
//! - a literal value set given to a flag (`--level module|symbol|both`) equals
//!   the snapshot's `values=`, and a single literal (`--source notion`) is one
//!   of them; a `<PLACEHOLDER>` is never checked.
//!
//! And every command path a snapshot declares with no subcommand below it
//! (`find`, `cache push`), unless it or a parent is `hidden` (`hook`), has at
//! least one usage line, and its usage lines together show every flag it
//! declares but `--help`, the global-propagated `--no-overlay` and the
//! hidden ones (README's "every subcommand and flag"). Description prose is
//! never read, so a help-text tweak cannot break this; a new command, a new,
//! renamed or removed flag or a changed value set does. A failure lists
//! `README.md:<line>` for each stale usage line and names each command, or
//! command and flag, with none.
//!
//! fired_on, on stderr:
//! `[readme-surface] commands=<n> usage_lines=<u> flags_checked=<f>`, where n
//! is the command paths that need a usage line, u the usage lines read and f
//! the flag mentions checked
//! (`cargo test -p glia-cli --test readme_surface -- --nocapture`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

struct ArgSpec {
    /// `--<long>`, or empty for a short-only flag.
    long: String,
    takes_value: bool,
    values: Vec<String>,
    hidden: bool,
    /// Propagated to every subcommand (`--no-overlay`).
    global: bool,
}

/// The CLI surface as the snapshots pin it. A section key is the command path
/// below `glia`, space-joined: `""` for the global options, `"find"`,
/// `"cache push"`.
#[derive(Default)]
struct Surface {
    flags: BTreeMap<String, BTreeMap<String, ArgSpec>>,
    /// Section -> (subcommand name or alias -> canonical name); `""` holds
    /// the top-level commands.
    subs: BTreeMap<String, BTreeMap<String, String>>,
    hidden: BTreeSet<String>,
}

impl Surface {
    fn load(dir: &Path) -> Surface {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "txt"))
            .collect();
        files.sort();
        let mut s = Surface::default();
        for f in &files {
            s.add_snapshot(&std::fs::read_to_string(f).expect("read a surface snapshot"));
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
                let (mut path, mut aliases, mut hidden) = (Vec::new(), Vec::new(), false);
                for word in rest.split_whitespace() {
                    match word {
                        "hidden" => hidden = true,
                        "subcommand_required" => {}
                        w => match w.strip_prefix("aliases=") {
                            Some(list) => aliases = list.split(',').collect(),
                            None => path.push(w),
                        },
                    }
                }
                section = path.join(" ");
                self.flags.entry(section.clone()).or_default();
                if hidden {
                    self.hidden.insert(section.clone());
                }
                if let Some((last, parent)) = path.split_last() {
                    let subs = self.subs.entry(parent.join(" ")).or_default();
                    for name in std::iter::once(*last).chain(aliases) {
                        subs.insert(name.to_string(), last.to_string());
                    }
                }
            } else if let Some(rest) = line.trim_start().strip_prefix("arg ") {
                let kv: BTreeMap<&str, &str> = rest
                    .split_whitespace()
                    .filter_map(|w| w.split_once('='))
                    .collect();
                let mut names = Vec::new();
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
                    flags.insert(
                        n,
                        ArgSpec {
                            long: kv
                                .get("long")
                                .filter(|l| **l != "-")
                                .map(|l| format!("--{l}"))
                                .unwrap_or_default(),
                            takes_value: kv.get("num_args").is_some_and(|n| *n != "0"),
                            values: kv
                                .get("values")
                                .map(|v| v.split(',').map(str::to_string).collect())
                                .unwrap_or_default(),
                            hidden: kv.get("hidden") == Some(&"true"),
                            global: kv.get("global") == Some(&"true"),
                        },
                    );
                }
            }
        }
    }

    /// The flag's spec in the innermost section of `path` declaring it, else
    /// the global options.
    fn flag(&self, path: &[String], name: &str) -> Option<&ArgSpec> {
        (0..=path.len())
            .rev()
            .find_map(|n| self.flags.get(&path[..n].join(" "))?.get(name))
    }

    fn has_subs(&self, path: &str) -> bool {
        self.subs.get(path).is_some_and(|s| !s.is_empty())
    }

    /// Hidden itself, or under a hidden command.
    fn is_hidden(&self, path: &[String]) -> bool {
        (1..=path.len()).any(|n| self.hidden.contains(&path[..n].join(" ")))
    }

    /// Every visible command path with no subcommand below it: each needs a
    /// usage line.
    fn leaves(&self) -> Vec<String> {
        self.flags
            .keys()
            .filter(|k| !k.is_empty() && !self.has_subs(k))
            .filter(|k| !self.is_hidden(&k.split(' ').map(str::to_string).collect::<Vec<_>>()))
            .cloned()
            .collect()
    }

    /// The flags a usage line of `leaf` must show: every long flag it
    /// declares but `--help`, the global-propagated and the hidden ones.
    fn shown_flags(&self, leaf: &str) -> Vec<&str> {
        self.flags.get(leaf).map_or_else(Vec::new, |flags| {
            flags
                .iter()
                .filter(|(n, a)| **n == a.long && *n != "--help" && !a.hidden && !a.global)
                .map(|(n, _)| n.as_str())
                .collect()
        })
    }
}

/// `(1-based line, text)` of every usage line: a line of a fenced block in
/// the `## CLI` section that starts with `glia ` at column 0.
fn usage_lines(md: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let (mut in_cli, mut in_fence) = (false, false);
    for (i, line) in md.lines().enumerate() {
        if !in_fence && line.starts_with("## ") {
            in_cli = line.trim_end() == "## CLI";
            continue;
        }
        if !in_cli {
            continue;
        }
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if in_fence && line.starts_with("glia ") {
            out.push((i + 1, line.to_string()));
        }
    }
    out
}

/// A usage word without its grouping: `[--with` -> `--with`, `<WITH>]...` ->
/// `<WITH>`.
fn core(word: &str) -> &str {
    let mut w = word.trim_start_matches(['[', '(']);
    loop {
        let t = w.trim_end_matches([']', ')', ',']);
        let t = t.strip_suffix("...").unwrap_or(t);
        if t == w {
            return w;
        }
        w = t;
    }
}

/// One usage line checked.
#[derive(Default)]
struct LineCheck {
    /// The command paths it names, canonical, one per `a|b` alternative.
    paths: Vec<Vec<String>>,
    /// Flag mentions checked.
    checked: usize,
    /// `(command path, --long)` per flag it shows that a path declares.
    shown: Vec<(String, String)>,
    /// One per problem.
    found: Vec<String>,
}

fn check_line(s: &Surface, line: &str) -> LineCheck {
    let words: Vec<&str> = line.split_whitespace().skip(1).map(core).collect();
    let mut paths: Vec<Vec<String>> = vec![Vec::new()];
    let (mut found, mut shown, mut checked) = (Vec::new(), Vec::new(), 0);
    let (mut resolving, mut i) = (true, 0);
    while i < words.len() {
        let w = words[i];
        i += 1;
        if w.is_empty() || w == "|" {
            continue;
        }
        if !(w.starts_with('-') && w.len() > 1) {
            let key = paths[0].join(" ");
            if resolving && s.has_subs(&key) && !w.starts_with('<') {
                let mut next = Vec::new();
                for alt in w.split('|') {
                    match s.subs[&key].get(alt) {
                        Some(canon) => next.push([paths[0].clone(), vec![canon.clone()]].concat()),
                        None if key.is_empty() => found.push(format!(
                            "unknown command `{alt}` (no cli/surface/{alt}.txt)"
                        )),
                        None => found.push(format!("unknown subcommand `glia {key} {alt}`")),
                    }
                }
                if next.is_empty() {
                    return LineCheck {
                        found,
                        ..LineCheck::default()
                    };
                }
                paths = next;
            } else {
                resolving = false;
            }
            continue;
        }
        resolving = false;
        checked += 1;
        let (name, inline) = match w.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (w, None),
        };
        let specs: Vec<Option<&ArgSpec>> = paths.iter().map(|p| s.flag(p, name)).collect();
        let Some(spec) = specs.iter().copied().flatten().next() else {
            found.push(format!(
                "unknown flag `{name}` for `glia {}`",
                paths[0].join(" ")
            ));
            continue;
        };
        for (p, a) in paths.iter().zip(&specs) {
            if let Some(a) = a.filter(|a| !a.long.is_empty()) {
                shown.push((p.join(" "), a.long.clone()));
            }
        }
        if specs.iter().any(Option::is_none) {
            found.push(format!(
                "`{name}` is not declared by every subcommand the line names"
            ));
        }
        if spec.hidden {
            found.push(format!("`{name}` is hidden in its snapshot"));
        }
        if !spec.takes_value {
            continue;
        }
        let value = inline.or_else(|| {
            let v = words
                .get(i)
                .copied()
                .filter(|v| *v != "|" && !v.starts_with("--"));
            i += usize::from(v.is_some());
            v
        });
        let Some(v) = value.filter(|v| !v.is_empty() && !v.starts_with('<')) else {
            continue;
        };
        let shown: BTreeSet<&str> = v.split('|').collect();
        let declared: BTreeSet<&str> = spec.values.iter().map(String::as_str).collect();
        let fits = if shown.len() > 1 {
            shown == declared
        } else {
            declared.is_empty() || declared.is_superset(&shown)
        };
        if !fits {
            found.push(format!(
                "`{name} {v}`: the snapshot's values are {}",
                if declared.is_empty() {
                    "open (no values=)".to_string()
                } else {
                    spec.values.join("|")
                }
            ));
        }
    }
    for p in &paths {
        if s.is_hidden(p) {
            found.push(format!("`glia {}` is hidden in its snapshot", p.join(" ")));
        } else if !p.is_empty() && s.has_subs(&p.join(" ")) {
            found.push(format!("names no subcommand of `glia {}`", p.join(" ")));
        }
    }
    LineCheck {
        paths,
        checked,
        shown,
        found,
    }
}

struct Report {
    commands: usize,
    usage_lines: usize,
    flags_checked: usize,
    /// `README.md:<line>: ...` per stale usage line, then one line per
    /// command with no usage line and per flag no usage line shows.
    failures: Vec<String>,
}

impl Report {
    fn marker(&self) -> String {
        format!(
            "[readme-surface] commands={} usage_lines={} flags_checked={}",
            self.commands, self.usage_lines, self.flags_checked
        )
    }
}

fn check_readme(s: &Surface, md: &str) -> Report {
    let lines = usage_lines(md);
    let (mut covered, mut shown) = (BTreeSet::new(), BTreeSet::new());
    let (mut failures, mut flags_checked) = (Vec::new(), 0);
    for (n, text) in &lines {
        let c = check_line(s, text);
        flags_checked += c.checked;
        covered.extend(c.paths.into_iter().map(|p| p.join(" ")));
        shown.extend(c.shown);
        failures.extend(
            c.found
                .into_iter()
                .map(|f| format!("README.md:{n}: `{text}`: {f}")),
        );
    }
    let leaves = s.leaves();
    for leaf in &leaves {
        let file = leaf.split(' ').next().unwrap_or(leaf);
        if !covered.contains(leaf) {
            failures.push(format!(
                "README.md ## CLI: no usage line for `glia {leaf}` (cli/surface/{file}.txt)"
            ));
            continue;
        }
        for flag in s.shown_flags(leaf) {
            if !shown.contains(&(leaf.clone(), flag.to_string())) {
                failures.push(format!(
                    "README.md ## CLI: no usage line of `glia {leaf}` shows `{flag}` (cli/surface/{file}.txt)"
                ));
            }
        }
    }
    Report {
        commands: leaves.len(),
        usage_lines: lines.len(),
        flags_checked,
        failures,
    }
}

fn surface() -> Surface {
    Surface::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("surface"))
}

fn readme() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("README.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn readme_cli_block_matches_the_cli_surface() {
    let r = check_readme(&surface(), &readme());
    eprintln!("{}", r.marker());
    assert!(r.usage_lines > 0 && r.commands > 0, "{}", r.marker());
    assert!(
        r.failures.is_empty(),
        "README.md `## CLI` is stale against cli/surface/*.txt ({} finding(s)); write each \
         usage line from its command's snapshot:\n{}",
        r.failures.len(),
        r.failures.join("\n")
    );
}

/// The negative case on the real README: a usage line with an undeclared flag
/// is reported at its line, and a command whose usage line is dropped is
/// reported missing.
#[test]
fn a_broken_readme_copy_is_reported() {
    let s = surface();
    let real = readme();
    let pack_line = real
        .lines()
        .position(|l| l.starts_with("glia pack "))
        .expect("README has a `glia pack` usage line");
    let mut lines: Vec<&str> = real.lines().collect();
    lines.remove(pack_line);
    let first = lines
        .iter()
        .position(|l| l.starts_with("glia "))
        .expect("README has a usage line");
    lines.insert(first + 1, "glia find <REPO> <QUERY> --nope");
    let broken = lines.join("\n");

    let r = check_readme(&s, &broken);
    assert_eq!(r.failures.len(), 2, "{}", r.failures.join("\n"));
    assert_eq!(
        r.failures[0],
        format!(
            "README.md:{}: `glia find <REPO> <QUERY> --nope`: unknown flag `--nope` for `glia find`",
            first + 2
        )
    );
    assert_eq!(
        r.failures[1],
        "README.md ## CLI: no usage line for `glia pack` (cli/surface/pack.txt)"
    );
}

/// Each kind of stale mention is reported at its line; well-formed lines, the
/// lines outside a fence or outside `## CLI`, and description prose are not.
#[test]
fn checker_reports_each_stale_mention_at_its_line() {
    let s = surface();
    let md = "\
## Install
```
glia fnd <REPO>
```
## CLI
glia fnd <REPO> outside the fence
```
# a group comment
glia fnd <REPO>
glia cache pusj <REPO> <STORE>
glia impact <REPO> <QNAME> [--direction forward|sideways]
glia hotspots <REPO> [--level module|symbol]
glia patterns <REPO> [--experimental]
glia hook pre-commit --pair <PAIR>
glia timeline build|history|as-of <REPO> [--json] [--revs <REVS>]
glia docs sync <REPO> --source wiki
glia cache
glia find <REPO> <QUERY> [--kind <KIND>]... [--top-k=5] [--json]
    glia find --nope is prose, not a usage line
glia docs sync <REPO> --source notion --database <ID> | --page <ID> [--with-bogus]
glia serves <REPO> <CHANNEL> [--mechanism auto|http|queue]
glia gaps <REPO> [--category <CATEGORY>] [--json]
```
## Roadmap
```
glia fnd <REPO>
```
";
    let r = check_readme(&s, md);
    let at: Vec<&str> = r
        .failures
        .iter()
        .filter_map(|f| f.split(": `").next())
        .filter(|l| l.starts_with("README.md:"))
        .collect();
    assert_eq!(
        at,
        [
            "README.md:9",
            "README.md:10",
            "README.md:11",
            "README.md:12",
            "README.md:13",
            "README.md:14",
            "README.md:15",
            "README.md:16",
            "README.md:17",
            "README.md:20",
        ],
        "{}",
        r.failures.join("\n")
    );
    let f = &r.failures;
    assert!(
        f[0].ends_with("unknown command `fnd` (no cli/surface/fnd.txt)"),
        "{}",
        f[0]
    );
    assert!(
        f[1].ends_with("unknown subcommand `glia cache pusj`"),
        "{}",
        f[1]
    );
    assert!(
        f[2].ends_with(
            "`--direction forward|sideways`: the snapshot's values are forward|backward|both"
        ),
        "{}",
        f[2]
    );
    assert!(f[3].contains("`--level module|symbol`"), "{}", f[3]);
    assert!(
        f[4].ends_with("`--experimental` is hidden in its snapshot"),
        "{}",
        f[4]
    );
    assert!(
        f[5].ends_with("`glia hook pre-commit` is hidden in its snapshot"),
        "{}",
        f[5]
    );
    assert!(
        f[6].ends_with("`--revs` is not declared by every subcommand the line names"),
        "{}",
        f[6]
    );
    assert!(f[7].contains("`--source wiki`"), "{}", f[7]);
    assert!(
        f[8].ends_with("names no subcommand of `glia cache`"),
        "{}",
        f[8]
    );
    assert!(
        f[9].ends_with("unknown flag `--with-bogus` for `glia docs sync`"),
        "{}",
        f[9]
    );
    assert_eq!(r.usage_lines, 13, "only fenced `glia ` lines inside ## CLI");
    assert!(
        f.iter()
            .any(|l| l.contains("no usage line for `glia pack`")),
        "a command with no line is named"
    );
    assert!(
        !f.iter()
            .any(|l| l.contains("glia hook") && l.contains("no usage line")),
        "hidden commands need no usage line"
    );
    let unshown = |cmd: &str, flag: &str| {
        f.iter().any(|l| {
            l.starts_with(&format!(
                "README.md ## CLI: no usage line of `glia {cmd}` shows `{flag}`"
            ))
        })
    };
    assert!(unshown("find", "--scope") && unshown("find", "--with"));
    assert!(
        !unshown("find", "--kind") && !unshown("find", "--top-k") && !unshown("find", "--json"),
        "a shown flag (`--top-k=5` too) counts: {}",
        f.join("\n")
    );
    assert!(!unshown("find", "--help") && !unshown("find", "--no-overlay"));
    assert!(unshown("timeline build", "--head") && !unshown("timeline build", "--revs"));
    assert!(unshown("timeline history", "--category") && !unshown("timeline as-of", "--json"));
    assert!(
        !unshown("patterns", "--experimental"),
        "hidden flags need no mention"
    );
}
