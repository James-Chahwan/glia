use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, cell_type, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};

pub struct CliEntrypoint {
    pub from: NodeId,
    pub framework: CliFramework,
    pub command_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliFramework {
    Click,
    Typer,
    Argparse,
    Cobra,
    Clap,
    Commander,
    Yargs,
    Thor,
    OptionParser,
}

pub fn extract_cli_entrypoints(source: &str, from: NodeId) -> Vec<CliEntrypoint> {
    let mut entries = Vec::new();
    for (pattern, framework) in PATTERNS {
        if source.contains(pattern) {
            entries.push(CliEntrypoint {
                from,
                framework: framework.clone(),
                command_name: pattern.to_string(),
            });
        }
    }
    entries
}

const PATTERNS: &[(&str, CliFramework)] = &[
    ("@click.command", CliFramework::Click),
    ("@click.group", CliFramework::Click),
    ("@app.command", CliFramework::Typer),
    ("typer.Typer", CliFramework::Typer),
    ("argparse.ArgumentParser", CliFramework::Argparse),
    ("cobra.Command", CliFramework::Cobra),
    ("rootCmd", CliFramework::Cobra),
    ("clap::Parser", CliFramework::Clap),
    ("#[command", CliFramework::Clap),
    (".command(", CliFramework::Commander),
    ("yargs(", CliFramework::Yargs),
    ("Thor", CliFramework::Thor),
    ("OptionParser", CliFramework::OptionParser),
];

pub struct CliNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
}

pub fn extract_cli_command_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> CliNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    let mut seen = std::collections::HashSet::new();

    let extractors: &[fn(&str) -> Option<String>] = &[
        extract_cobra_command_name,
        extract_click_command_name,
        extract_commander_command_name,
        extract_clap_command_name,
    ];
    for line in source.lines() {
        let trimmed = line.trim();
        for extractor in extractors {
            if let Some(name) = extractor(trimmed)
                && seen.insert(name.clone())
            {
                add_cli_command(&mut nodes, &mut nav, &name, module_id, repo);
                break;
            }
        }
    }

    CliNodes { nodes, nav }
}

fn add_cli_command(
    nodes: &mut Vec<Node>,
    nav: &mut CodeNav,
    name: &str,
    module_id: NodeId,
    repo: RepoId,
) {
    let qname = format!("cli:{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLI_COMMAND, &qname);
    nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: vec![],
    });
    nav.record(id, name, &qname, node_kind::CLI_COMMAND, Some(module_id));
}

fn extract_cobra_command_name(line: &str) -> Option<String> {
    if !line.contains("cobra.Command") {
        return None;
    }
    let use_idx = line.find("Use:")?;
    let after = &line[use_idx + 4..];
    extract_quoted(after.trim_start())
}

fn extract_click_command_name(line: &str) -> Option<String> {
    let rest = line.strip_prefix("@click.command(")?;
    extract_quoted(rest.trim_start())
}

fn extract_commander_command_name(line: &str) -> Option<String> {
    if !line.contains(".command(") {
        return None;
    }
    let idx = line.find(".command(")?;
    let after = &line[idx + 9..];
    extract_quoted(after.trim_start())
}

fn extract_clap_command_name(line: &str) -> Option<String> {
    if !line.contains("#[command(name") {
        return None;
    }
    let idx = line.find("name")?;
    let after = &line[idx + 4..];
    let eq = after.find('=')?;
    extract_quoted(after[eq + 1..].trim_start())
}

fn extract_quoted(s: &str) -> Option<String> {
    let (quote, rest) = if let Some(rest) = s.strip_prefix('"') {
        ('"', rest)
    } else if let Some(rest) = s.strip_prefix('\'') {
        ('\'', rest)
    } else {
        return None;
    };
    let end = rest.find(quote)?;
    let lit = &rest[..end];
    if lit.is_empty() || lit.len() > 64 {
        return None;
    }
    Some(lit.to_string())
}

const INVOCATION_PATTERNS: &[&str] = &[
    "exec.Command(",
    "exec.CommandContext(",
    "child_process.spawn(",
    "child_process.exec(",
    "child_process.execFile(",
    "execSync(",
    "spawnSync(",
    "subprocess.run(",
    "subprocess.Popen(",
    "subprocess.call(",
    "os.system(",
    "Process.Start(",
    "std::process::Command::new(",
    "Command::new(",
    "system(",
];

/// Most argv tokens [`scan_argv`] reads from one call site (the binary included).
const MAX_ARGV_TOKENS: usize = 8;

/// A13.4 — every CLI_INVOCATION in a file, keyed by the BINARY (`argv[0]`), so
/// it pairs with the `cli:<name>` a cobra/click/clap root declares. The rest of
/// each call's argv is kept on the node's argv cell ([`argv_cell`]), which is
/// how the resolver reaches the subcommand (`mytool migrate` -> `cli:migrate`).
///
/// One node per binary per file: a second `exec.Command("docker", "push")`
/// adds its argv vector to the cell instead of being lost to the dedupe.
pub fn extract_cli_invocation_nodes(
    source: &str,
    module_id: NodeId,
    repo: RepoId,
) -> CliNodes {
    // Keyed by the byte offset the arguments start at: `os.system(` and
    // `system(` (or `Command::new(` inside `std::process::Command::new(`) end at
    // the same `(`, so overlapping needles collapse to one call site, and the
    // iteration is in source order.
    let mut sites: std::collections::BTreeMap<usize, Vec<String>> = std::collections::BTreeMap::new();
    for &pattern in INVOCATION_PATTERNS {
        let mut search_from = 0;
        while let Some(rel_idx) = source[search_from..].find(pattern) {
            let args_at = search_from + rel_idx + pattern.len();
            if !sites.contains_key(&args_at) {
                let tokens = scan_argv(&source[args_at..]);
                if !tokens.is_empty() {
                    sites.insert(args_at, tokens);
                }
            }
            search_from = args_at;
        }
    }

    // BTreeMap: nodes come out sorted by binary, whatever order the needles hit.
    let mut by_bin: std::collections::BTreeMap<String, Vec<Vec<String>>> =
        std::collections::BTreeMap::new();
    for tokens in sites.into_values() {
        let Some((head, args)) = tokens.split_first() else { continue };
        let Some(bin) = binary_name(head) else { continue };
        let argv = by_bin.entry(bin).or_default();
        if !args.is_empty() && !argv.iter().any(|seen| seen.as_slice() == args) {
            argv.push(args.to_vec());
        }
    }

    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();
    for (bin, argv) in &by_bin {
        let qname = format!("cli_invoke:{bin}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLI_INVOCATION, &qname);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: vec![argv_cell(bin, argv)],
        });
        nav.record(id, bin, &qname, node_kind::CLI_INVOCATION, Some(module_id));
    }

    CliNodes { nodes, nav }
}

/// The quoted argv at a call site, from the text right after an
/// [`INVOCATION_PATTERNS`] needle: `("docker", "build")`, `(["tool", "migrate"])`
/// (Python list), `("git", ["status"])` (JS/Ruby argv array), `([]string{..})`.
///
/// Skips leading whitespace and at most ONE opening bracket — before the first
/// token (`[`, `(`, `{`, Go `[]string{`) or, after it, an argv array (`[`, Go
/// `[]string{`). Then reads up to [`MAX_ARGV_TOKENS`] quoted tokens separated only
/// by whitespace, `,` and `.` (Go `...`), stopping at anything else — a closing
/// bracket, an identifier, a `+`, a `:`. A first token holding whitespace is a
/// shell string (`subprocess.run('terraform apply')`) and is split on it.
fn scan_argv(after: &str) -> Vec<String> {
    let mut rest = after.trim_start();
    let mut bracket_open = false;
    if let Some(r) = strip_open_bracket(rest, &["[]string{", "[", "(", "{"]) {
        rest = r.trim_start();
        bracket_open = true;
    }
    let mut tokens: Vec<String> = Vec::new();
    while tokens.len() < MAX_ARGV_TOKENS {
        let Some(tok) = extract_quoted(rest) else { break };
        // Opening quote + literal + closing quote, all ASCII delimiters.
        rest = &rest[tok.len() + 2..];
        tokens.push(tok);
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',' || c == '.');
        if !bracket_open && let Some(r) = strip_open_bracket(rest, &["[]string{", "["]) {
            rest = r.trim_start();
            bracket_open = true;
        }
    }
    if tokens.first().is_some_and(|t| t.contains(|c: char| c.is_ascii_whitespace())) {
        let head = tokens.remove(0);
        let mut split: Vec<String> = head.split_ascii_whitespace().map(str::to_string).collect();
        if split.is_empty() {
            // `" "`: a blank first token names no binary — do not promote argv[1].
            return Vec::new();
        }
        split.append(&mut tokens);
        split.truncate(MAX_ARGV_TOKENS);
        tokens = split;
    }
    tokens
}

fn strip_open_bracket<'a>(s: &'a str, openers: &[&str]) -> Option<&'a str> {
    openers.iter().find_map(|o| s.strip_prefix(o))
}

/// `argv[0]` as a binary name: the last path segment (`/usr/bin/terraform` ->
/// `terraform`, `C:\tools\x.exe` -> `x`). `None` for an empty or over-long name,
/// or an unresolved interpolation (`$TOOL`, `{tool}`) — that is not a binary.
fn binary_name(argv0: &str) -> Option<String> {
    let base = argv0.rsplit(['/', '\\']).next().unwrap_or(argv0);
    let base = base.strip_suffix(".exe").unwrap_or(base);
    if base.is_empty() || base.len() > 64 || base.contains(['$', '{']) {
        return None;
    }
    Some(base.to_string())
}

/// The argv cell's JSON shape. Field order is the serialised order, so the
/// payload is byte-stable.
#[derive(serde::Serialize, serde::Deserialize)]
struct ArgvCellPayload<'a> {
    #[serde(borrow)]
    bin: std::borrow::Cow<'a, str>,
    #[serde(default)]
    argv: Vec<Vec<String>>,
}

/// A13.4 — the CODE cell a CLI_INVOCATION carries: the binary and every
/// distinct non-empty argument vector (argv without `argv[0]`) seen for it in
/// one file, e.g. `{"bin":"docker","argv":[["build","."],["push","x"]]}`. A
/// binary invoked from N files carries N cells. Written only here and read only
/// by [`parse_argv_cell`], so the format lives in one place.
pub fn argv_cell(bin: &str, argv: &[Vec<String>]) -> Cell {
    let payload = ArgvCellPayload { bin: std::borrow::Cow::Borrowed(bin), argv: argv.to_vec() };
    Cell {
        kind: cell_type::CODE,
        payload: CellPayload::Json(serde_json::to_string(&payload).unwrap_or_default()),
    }
}

/// The argument vectors of one [`argv_cell`]. Empty for any other cell, or a
/// payload that does not parse.
pub fn parse_argv_cell(cell: &Cell) -> Vec<Vec<String>> {
    match &cell.payload {
        CellPayload::Json(json) if cell.kind == cell_type::CODE => {
            serde_json::from_str::<ArgvCellPayload>(json).map(|p| p.argv).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }
    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test")
    }

    #[test]
    fn detects_click() {
        let id = module_id();
        let refs = extract_cli_entrypoints("@click.command()\ndef run():", id);
        assert!(refs.iter().any(|r| r.framework == CliFramework::Click));
    }

    #[test]
    fn detects_cobra() {
        let id = module_id();
        let refs = extract_cli_entrypoints("var rootCmd = &cobra.Command{}", id);
        assert!(refs.iter().any(|r| r.framework == CliFramework::Cobra));
    }

    #[test]
    fn cobra_command_node() {
        let source = r#"var cmd = &cobra.Command{Use: "migrate"}"#;
        let result = extract_cli_command_nodes(source, module_id(), repo());
        assert!(result.nav.qname_by_id.values().any(|q| q == "cli:migrate"));
    }

    #[test]
    fn commander_command_node() {
        let source = "program.command('deploy').description('Deploy app')";
        let result = extract_cli_command_nodes(source, module_id(), repo());
        assert!(result.nav.qname_by_id.values().any(|q| q == "cli:deploy"));
    }

    #[test]
    fn detects_exec_command_invocation() {
        let source = "cmd := exec.Command(\"docker\", \"build\", \".\")";
        let result = extract_cli_invocation_nodes(source, module_id(), repo());
        assert!(result.nav.qname_by_id.values().any(|q| q == "cli_invoke:docker"));
    }

    /// The single qname and the argv vectors of its cell(s).
    fn invocation(source: &str) -> (Vec<String>, Vec<Vec<String>>) {
        let result = extract_cli_invocation_nodes(source, module_id(), repo());
        let qnames = result
            .nodes
            .iter()
            .filter_map(|n| result.nav.qname_by_id.get(&n.id).cloned())
            .collect();
        let argv = result.nodes.iter().flat_map(|n| n.cells.iter().flat_map(parse_argv_cell)).collect();
        (qnames, argv)
    }

    #[test]
    fn detects_subprocess_invocation() {
        // A shell string is split: the key is the binary, not `terraform apply`.
        let (qnames, argv) = invocation("subprocess.run('terraform apply')");
        assert_eq!(qnames, vec!["cli_invoke:terraform"]);
        assert_eq!(argv, vec![vec!["apply"]]);
    }

    #[test]
    fn detects_list_form_invocation() {
        let (qnames, argv) = invocation("subprocess.run([\"terraform\", \"apply\"])");
        assert_eq!(qnames, vec!["cli_invoke:terraform"]);
        assert_eq!(argv, vec![vec!["apply"]]);
    }

    #[test]
    fn detects_go_string_slice_invocation() {
        let (qnames, argv) = invocation("cmd := exec.Command(\"kubectl\", \"apply\", \"-f\", p)");
        assert_eq!(qnames, vec!["cli_invoke:kubectl"]);
        assert_eq!(argv, vec![vec!["apply", "-f"]]);
    }

    #[test]
    fn repeated_binary_accumulates_argv_in_one_node() {
        let source = "exec.Command(\"docker\", \"build\", \".\")\nexec.Command(\"docker\", \"push\", \"x\")\nexec.Command(\"docker\", \"build\", \".\")";
        let result = extract_cli_invocation_nodes(source, module_id(), repo());
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].cells.len(), 1);
        let CellPayload::Json(json) = &result.nodes[0].cells[0].payload else { panic!("argv cell is JSON") };
        assert_eq!(json, r#"{"bin":"docker","argv":[["build","."],["push","x"]]}"#);
    }

    #[test]
    fn path_qualified_binary_keys_on_basename() {
        assert_eq!(invocation("subprocess.run(['/usr/bin/kubectl', 'get'])").0, vec!["cli_invoke:kubectl"]);
        assert_eq!(invocation("Process.Start(\"C:\\\\tools\\\\deploy.exe\")").0, vec!["cli_invoke:deploy"]);
    }

    #[test]
    fn js_argv_array_after_binary_is_read() {
        let (qnames, argv) = invocation("child_process.spawn('git', ['status', '--short'], opts)");
        assert_eq!(qnames, vec!["cli_invoke:git"]);
        assert_eq!(argv, vec![vec!["status", "--short"]]);
    }

    #[test]
    fn overlapping_needles_are_one_call_site() {
        // `system(` also matches inside `os.system(`: one call, one argv vector.
        let (qnames, argv) = invocation("os.system(\"make test\")");
        assert_eq!(qnames, vec!["cli_invoke:make"]);
        assert_eq!(argv, vec![vec!["test"]]);
    }

    #[test]
    fn interpolated_or_unquoted_binary_is_not_minted() {
        assert!(invocation("subprocess.run(['$TOOL', 'x'])").0.is_empty());
        assert!(invocation("exec.Command(\"{tool}\")").0.is_empty());
        assert!(invocation("exec.Command(tool, \"x\")").0.is_empty());
        assert!(invocation("subprocess.run([\" \", \"x\"])").0.is_empty());
    }

    #[test]
    fn argv_cell_round_trips_and_ignores_other_cells() {
        let argv = vec![vec!["migrate".to_string(), "--yes".to_string()]];
        assert_eq!(parse_argv_cell(&argv_cell("mytool", &argv)), argv);
        let text = Cell { kind: cell_type::CODE, payload: CellPayload::Text("x".into()) };
        assert!(parse_argv_cell(&text).is_empty());
    }
}
