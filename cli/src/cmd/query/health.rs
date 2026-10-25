//! `glia health` (FH.2b): the human surface over the engine's
//! `health::health_with` (FH.2a). Whether an answer about this repo can be
//! trusted, before asking one: per boundary mechanism (HTTP, gRPC, RPC,
//! GraphQL, queues, WebSocket) how much of the client and server side the
//! graph pairs, then the code files no parser read, the workspace packages
//! imported by name and bound to no in-repo module, the suspicious ENDPOINT /
//! ROUTE qnames and the share of symbols `gaps` calls dead.
//!
//! The text form is a heading, the overall verdict line (the worst row and
//! why), the role line, a mechanisms table and one short section per other
//! measure; a section with nothing to say is left out. `--json` is the whole
//! engine `Health`, field for field. It reads the built graph plus one gated
//! walk of each repo root (the files no parser takes) and writes nothing. A
//! report: exits 0 whatever the verdict, 2 on a build error. The engine's
//! `[health] repo=<label> .. surface=cli` stderr line, once per call, is the
//! fired_on marker.

use std::path::PathBuf;

use glia_engine::health::{Health, HealthOptions, MechanismHealth, health_with};

use crate::common::generate_for;

/// Importer files listed per unresolved workspace package in the text form.
const IMPORTERS_SHOWN: usize = 3;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (their clients and servers pair across
    /// repos). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Emit the health report as JSON.
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let roots: Vec<(u64, PathBuf)> = result
        .repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect();
    let mut opts = HealthOptions::default();
    opts.labels = result.repo_labels.clone();
    opts.surface = "cli";
    let h = health_with(&result.merged, &roots, &opts);
    if args.json {
        match serde_json::to_string(&h) {
            Ok(s) => println!("{s}"),
            Err(e) => {
                eprintln!("error: {e}");
                return 2;
            }
        }
        return 0;
    }
    print!("{}", render(&args.repo, args.with.len(), &h));
    0
}

/// The whole text form.
fn render(repo: &str, others: usize, h: &Health) -> String {
    let mut out = String::new();
    let more = match others {
        0 => String::new(),
        1 => " (+1 repo)".to_string(),
        n => format!(" (+{n} repos)"),
    };
    out.push_str(&format!("# glia health {repo}{more}\n\n"));
    out.push_str(&format!("verdict: {} - {}\n", h.verdict.as_str(), h.reason));
    if h.role_basis.is_empty() {
        out.push_str(&format!("role: {}\n", h.role.as_str()));
    } else {
        out.push_str(&format!(
            "role: {} (from {})\n",
            h.role.as_str(),
            h.role_basis.join(", ")
        ));
    }
    out.push('\n');
    out.push_str(&mechanisms(&h.mechanisms));
    out.push_str(&parse_section(h));
    out.push_str(&workspace_section(h));
    if h.suspicious.count > 0 {
        out.push_str(&format!(
            "## suspicious endpoint / route qnames ({})\n\n",
            h.suspicious.count
        ));
        for q in &h.suspicious.examples {
            out.push_str(&format!("- {q}\n"));
        }
        if h.suspicious.count > h.suspicious.examples.len() {
            out.push_str(&format!(
                "… {} more\n",
                h.suspicious.count - h.suspicious.examples.len()
            ));
        }
        out.push('\n');
    }
    out.push_str(&format!(
        "dead symbols: {} of {} ({})\n",
        h.dead.dead_symbols,
        h.dead.symbols,
        pct(h.dead.share)
    ));
    out
}

/// `## mechanisms` and one table row per mechanism, in the engine's order.
fn mechanisms(rows: &[MechanismHealth]) -> String {
    let mut out = String::from("## mechanisms\n\n");
    out.push_str(
        "| mechanism | verdict | reason | clients | external | servers | pairs | client reach | server reach | measure |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|---|\n");
    for m in rows {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            m.mechanism,
            m.verdict.as_str(),
            m.reason,
            m.producers,
            m.external_producers,
            m.consumers,
            m.pairs,
            pct(m.client_reach),
            pct(m.server_reach),
            if m.measure.is_empty() {
                "-"
            } else {
                m.measure.as_str()
            },
        ));
    }
    out.push('\n');
    out
}

/// `parse: <V> <reason> (<measure>)`, then the unparsed code and prose
/// files by extension when there are any.
fn parse_section(h: &Health) -> String {
    let p = &h.parse;
    let mut out = format!("parse: {} {}", p.verdict.as_str(), p.reason);
    if !p.measure.is_empty() {
        out.push_str(&format!(" ({})", p.measure));
    }
    out.push('\n');
    let by_ext = |m: &std::collections::BTreeMap<String, usize>| -> String {
        m.iter()
            .map(|(ext, n)| format!(".{ext} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if p.unparsed_files > 0 {
        out.push_str(&format!(
            "  code no parser read: {} files ({})\n",
            p.unparsed_files,
            by_ext(&p.unparsed)
        ));
    }
    if p.docs_unparsed_files > 0 {
        out.push_str(&format!(
            "  prose no parser read (not a flag): {} files ({})\n",
            p.docs_unparsed_files,
            by_ext(&p.docs_unparsed)
        ));
    }
    out.push('\n');
    out
}

/// `## workspace imports unresolved (<n>)`: each package, its ecosystem and
/// directory, its importer count and the first [`IMPORTERS_SHOWN`].
fn workspace_section(h: &Health) -> String {
    if h.workspace_unresolved.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "## workspace imports unresolved ({})\n\n",
        h.workspace_unresolved.len()
    );
    for w in &h.workspace_unresolved {
        let shown: Vec<&str> = w
            .importers
            .iter()
            .take(IMPORTERS_SHOWN)
            .map(String::as_str)
            .collect();
        let rest = w.importers.len().saturating_sub(shown.len());
        let tail = if rest > 0 {
            format!(", … {rest} more")
        } else {
            String::new()
        };
        out.push_str(&format!(
            "- {} ({}, {}): {} importers ({}{tail})\n",
            w.package,
            w.ecosystem,
            w.path,
            w.importers.len(),
            shown.join(", ")
        ));
    }
    out.push('\n');
    out
}

/// A ratio as a whole percent, `-` when there is none.
fn pct(r: Option<f64>) -> String {
    r.map_or_else(
        || "-".to_string(),
        |x| format!("{}%", (x * 100.0).round() as i64),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    use glia_engine::generate_one;
    use glia_engine::health::{Role, Verdict};

    /// A fresh temp root, removed on drop.
    struct Root(PathBuf);

    impl Root {
        fn new(tag: &str) -> Root {
            let dir = std::env::temp_dir().join(format!(
                "glia-health-cli-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("temp root");
            Root(dir)
        }
        fn write(&self, rel: &str, body: &str) {
            let path = self.0.join(rel);
            if let Some(p) = path.parent() {
                std::fs::create_dir_all(p).expect("mkdir");
            }
            std::fs::write(path, body).expect("write");
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn health_of(root: &Path) -> Health {
        let r = generate_one(root.to_str().expect("utf-8 path")).expect("builds");
        let roots: Vec<(u64, PathBuf)> = r
            .repo_roots
            .iter()
            .map(|(id, p)| (*id, PathBuf::from(p)))
            .collect();
        let mut opts = HealthOptions::default();
        opts.labels = r.repo_labels.clone();
        opts.surface = "cli";
        health_with(&r.merged, &roots, &opts)
    }

    /// A TypeScript library with no boundary node: role library, every
    /// mechanism OK, and the text form says so row by row.
    #[test]
    fn library_renders_every_mechanism() {
        let root = Root::new("lib");
        root.write(
            "package.json",
            "{\"name\": \"strutil\", \"version\": \"1.0.0\"}\n",
        );
        root.write(
            "src/index.ts",
            "export function pad(s: string): string {\n  return ' ' + s;\n}\n",
        );
        let h = health_of(&root.0);
        assert_eq!(h.role, Role::Library, "{h:?}");
        assert_eq!(h.verdict, Verdict::Ok, "{h:?}");
        let text = render("lib", 0, &h);
        assert!(
            text.starts_with("# glia health lib\n\nverdict: OK - "),
            "{text}"
        );
        assert!(text.contains("\nrole: library\n"), "{text}");
        for m in ["http", "grpc", "rpc", "graphql", "queue", "ws"] {
            assert!(
                text.lines()
                    .any(|l| l.starts_with(&format!("| {m} | OK | library |"))),
                "{m}: {text}"
            );
        }
        assert!(text.contains("\nparse: OK "), "{text}");
        assert!(text.contains("\ndead symbols: "), "{text}");
        assert!(!text.contains("## workspace imports"), "{text}");
        assert!(!text.contains("## suspicious"), "{text}");
    }

    /// Unparsed code and prose are listed by extension, the prose marked as
    /// no flag; `--with` counts in the heading.
    #[test]
    fn unparsed_files_by_extension() {
        let root = Root::new("unparsed");
        root.write("src/main.py", "def main():\n    return 1\n");
        root.write("ui/App.marko", "<p>x</p>\n");
        root.write("docs/guide.mdx", "# Guide\n");
        let h = health_of(&root.0);
        assert_eq!(
            (h.parse.unparsed_files, h.parse.docs_unparsed_files),
            (1, 1),
            "{:?}",
            h.parse
        );
        let text = render("app", 2, &h);
        assert!(text.starts_with("# glia health app (+2 repos)\n"), "{text}");
        assert!(
            text.contains("  code no parser read: 1 files (.marko 1)\n"),
            "{text}"
        );
        assert!(
            text.contains("  prose no parser read (not a flag): 1 files (.mdx 1)\n"),
            "{text}"
        );
    }

    #[test]
    fn pct_cell() {
        assert_eq!(pct(None), "-");
        assert_eq!(pct(Some(0.0114)), "1%");
        assert_eq!(pct(Some(1.0)), "100%");
    }
}
