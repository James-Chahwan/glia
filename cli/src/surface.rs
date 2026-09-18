//! CLI surface snapshot test (LG.6a).
//!
//! Layout: one snapshot per top-level subcommand at `cli/surface/<command>.txt`
//! (kebab-case command name, e.g. `blast-radius.txt`), plus
//! `cli/surface/_global.txt` for `Cli`'s global options. No crate-name header
//! inside the files, so a crate rename touches only the snapshots of the
//! renamed binary. The packet that owns a command's module file regenerates
//! that command's snapshot in the same commit as the surface change:
//!
//! ```text
//! GLIA_UPDATE_SURFACE=1 cargo test --manifest-path cli/Cargo.toml cli_surface
//! ```
//!
//! Each file opens with one `#` line naming that command, then the lines
//! below; the header is rendered too, so it is compared like the rest.
//!
//! What is pinned: every (sub)command path with its `hidden` / alias /
//! `subcommand_required` settings, and one line per argument — id, position,
//! long, short, required-ness, `num_args`, action, defaults, value names,
//! possible values, aliases, global, hidden. Help text is deliberately NOT
//! pinned (a doc tweak is not an API change).
//!
//! Rendered from `Cli::command()` (whose name is set to `glia`), never by
//! running the binary: clap's Usage line takes the bin name from argv[0], so a
//! renamed test binary would otherwise leak into the snapshot. clap's
//! generated `help` subcommand is skipped: with the help tree expanded it
//! mirrors every other command, so adding one command would touch a second
//! snapshot. The `--help` / `--version` flags clap adds ARE pinned.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use clap::{Arg, ArgAction, Command, CommandFactory};

const UPDATE_ENV: &str = "GLIA_UPDATE_SURFACE";
const REGENERATE: &str =
    "GLIA_UPDATE_SURFACE=1 cargo test --manifest-path cli/Cargo.toml cli_surface";
const GLOBAL_STEM: &str = "_global";

fn surface_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("surface")
}

/// Every snapshot file's stem → its rendered contents, for the real `Cli`.
fn render_surface() -> BTreeMap<String, String> {
    render_command(super::Cli::command())
}

fn render_command(mut root: Command) -> BTreeMap<String, String> {
    root.build();
    let mut files = BTreeMap::new();

    let root_path = [root.get_name().to_string()];
    let mut global = header(&format!("`{}` global options", root_path[0]));
    render_command_header(&mut global, &root, &root_path);
    render_args(&mut global, &root);
    files.insert(GLOBAL_STEM.to_string(), global);

    for sub in user_subcommands(&root) {
        let path = [root_path[0].clone(), sub.get_name().to_string()];
        let mut out = header(&format!("`{}`", path.join(" ")));
        render_tree(&mut out, sub, &path);
        files.insert(sub.get_name().to_string(), out);
    }
    files
}

fn header(what: &str) -> String {
    format!("# CLI surface of {what}. Regenerate in the commit that changes it: {REGENERATE}\n")
}

/// Subcommands a user declared, sorted by name — clap's generated `help`
/// subcommand is left out (see the module doc).
fn user_subcommands(cmd: &Command) -> Vec<&Command> {
    let mut subs: Vec<&Command> = cmd
        .get_subcommands()
        .filter(|s| cmd.is_disable_help_subcommand_set() || s.get_name() != "help")
        .collect();
    subs.sort_by(|a, b| a.get_name().cmp(b.get_name()));
    subs
}

fn render_tree(out: &mut String, cmd: &Command, path: &[String]) {
    render_command_header(out, cmd, path);
    render_args(out, cmd);
    for sub in user_subcommands(cmd) {
        let mut child = path.to_vec();
        child.push(sub.get_name().to_string());
        render_tree(out, sub, &child);
    }
}

fn render_command_header(out: &mut String, cmd: &Command, path: &[String]) {
    let mut line = format!("cmd {}", path.join(" "));
    if cmd.is_hide_set() {
        line.push_str(" hidden");
    }
    let mut aliases: Vec<&str> = cmd.get_all_aliases().collect();
    aliases.sort_unstable();
    if !aliases.is_empty() {
        let _ = write!(line, " aliases={}", aliases.join(","));
    }
    if cmd.is_subcommand_required_set() {
        line.push_str(" subcommand_required");
    }
    out.push_str(&line);
    out.push('\n');
}

fn render_args(out: &mut String, cmd: &Command) {
    let mut args: Vec<&Arg> = cmd.get_arguments().collect();
    args.sort_by(|a, b| a.get_id().as_str().cmp(b.get_id().as_str()));
    for arg in args {
        out.push_str(&render_arg(arg));
        out.push('\n');
    }
}

fn or_dash(parts: Vec<String>) -> String {
    if parts.is_empty() {
        "-".to_string()
    } else {
        parts.join(",")
    }
}

fn render_arg(arg: &Arg) -> String {
    let pos = arg
        .get_index()
        .map_or_else(|| "-".to_string(), |i| i.to_string());
    let long = arg.get_long().unwrap_or("-");
    let short = arg
        .get_short()
        .map_or_else(|| "-".to_string(), |c| c.to_string());
    let num_args = arg
        .get_num_args()
        .map_or_else(|| "-".to_string(), |r| r.to_string());
    let default = or_dash(
        arg.get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect(),
    );
    let value_names = or_dash(
        arg.get_value_names()
            .unwrap_or_default()
            .iter()
            .map(|v| v.to_string())
            .collect(),
    );
    let mut line = format!(
        "  arg {id} pos={pos} long={long} short={short} required={req} num_args={num_args} \
         action={action:?} default={default} value_names={value_names} global={global} hidden={hidden}",
        id = arg.get_id(),
        req = arg.is_required_set(),
        action = arg.get_action(),
        global = arg.is_global_set(),
        hidden = arg.is_hide_set(),
    );
    // Possible values only mean something for an arg that takes a value: a
    // `SetTrue` flag's bool parser would otherwise list `true,false`.
    if arg.get_action().takes_values() {
        let values: Vec<String> = arg
            .get_possible_values()
            .iter()
            .map(|v| v.get_name().to_string())
            .collect();
        if !values.is_empty() {
            let _ = write!(line, " values={}", values.join(","));
        }
    }
    let mut aliases: Vec<&str> = arg.get_all_aliases().unwrap_or_default();
    aliases.sort_unstable();
    if !aliases.is_empty() {
        let _ = write!(line, " aliases={}", aliases.join(","));
    }
    // No `env=`: clap's `env` feature is off, so no arg can read one. A packet
    // that turns it on adds `Arg::get_env` here.
    line
}

/// Changed lines only, `-` for the committed snapshot and `+` for the source,
/// in file order (an LCS walk; the files are a few dozen lines).
fn line_diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = String::new();
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            let _ = writeln!(out, "-{}", a[i]);
            i += 1;
        } else {
            let _ = writeln!(out, "+{}", b[j]);
            j += 1;
        }
    }
    out
}

/// Snapshot files on disk: stem → contents.
fn read_snapshots(dir: &Path) -> BTreeMap<String, String> {
    let mut on_disk = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return on_disk;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let body = std::fs::read_to_string(&path).unwrap_or_default();
        on_disk.insert(stem.to_string(), body);
    }
    on_disk
}

#[test]
fn cli_surface_matches_snapshot() {
    let dir = surface_dir();
    let rendered = render_surface();
    let on_disk = read_snapshots(&dir);

    if std::env::var(UPDATE_ENV).as_deref() == Ok("1") {
        std::fs::create_dir_all(&dir).expect("create cli/surface");
        for stem in on_disk.keys().filter(|s| !rendered.contains_key(*s)) {
            std::fs::remove_file(dir.join(format!("{stem}.txt"))).expect("remove stale snapshot");
        }
        for (stem, body) in &rendered {
            if on_disk.get(stem) != Some(body) {
                std::fs::write(dir.join(format!("{stem}.txt")), body).expect("write snapshot");
            }
        }
        return;
    }

    let mut report = String::new();
    for (stem, body) in &rendered {
        match on_disk.get(stem) {
            None => {
                let _ = writeln!(
                    report,
                    "cli/surface/{stem}.txt: missing\n{}",
                    line_diff("", body)
                );
            }
            Some(committed) if committed != body => {
                let _ = writeln!(
                    report,
                    "cli/surface/{stem}.txt: differs\n{}",
                    line_diff(committed, body)
                );
            }
            Some(_) => {}
        }
    }
    for stem in on_disk.keys().filter(|s| !rendered.contains_key(*s)) {
        let _ = writeln!(report, "cli/surface/{stem}.txt: stale (no such command)");
    }
    assert!(
        report.is_empty(),
        "CLI surface changed. If intended, regenerate in the same commit:\n  {REGENERATE}\n\n{report}"
    );
}

#[test]
fn cli_surface_renders_every_top_level_command() {
    let rendered = render_surface();
    for cmd in ["analyze", "blast-radius", "docs", "install-hooks", "trace"] {
        assert!(
            rendered.contains_key(cmd),
            "no snapshot rendered for `{cmd}`"
        );
    }
    assert!(
        !rendered.contains_key("help"),
        "clap's help subcommand leaked in"
    );
    let docs = &rendered["docs"];
    assert!(docs.contains("cmd glia docs sync\n"), "{docs}");
    assert!(docs.contains("cmd glia docs push\n"), "{docs}");
}

/// Falsifiability, kept as a test: one new flag on `trace` (what a
/// `#[arg(long)] x: bool` on its `Args` derives to) changes exactly one line
/// of exactly one snapshot.
#[test]
fn cli_surface_new_flag_shows_as_one_added_line() {
    let base = render_surface();
    let mutated = render_command(super::Cli::command().mut_subcommand("trace", |c| {
        c.arg(Arg::new("x").long("x").action(ArgAction::SetTrue))
    }));
    let changed: Vec<&str> = base
        .keys()
        .filter(|k| base.get(*k) != mutated.get(*k))
        .map(String::as_str)
        .collect();
    assert_eq!(changed, ["trace"]);
    let diff = line_diff(&base["trace"], &mutated["trace"]);
    assert_eq!(diff.lines().count(), 1, "{diff}");
    assert!(
        diff.starts_with(
            "+  arg x pos=- long=x short=- required=false num_args=0 action=SetTrue default=false "
        ),
        "{diff}"
    );
}

#[test]
fn cli_surface_line_diff_reports_only_changed_lines() {
    let diff = line_diff("a\nb\nc\n", "a\nx\nc\nd\n");
    assert_eq!(diff, "-b\n+x\n+d\n");
}
