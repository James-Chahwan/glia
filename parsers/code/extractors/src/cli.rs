use glia_code_domain::{
    CallQualifier, CodeNav, GRAPH_TYPE, UnresolvedRef, cell_type, edge_category, line_of, node_kind,
};
use glia_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};

/// The CLI library a `cli:<name>` declaration was read from: the
/// discriminator of the per-file `[cli-decl]` marker (`rust=clap:3`). A
/// variant is added in the same commit as the scanner that emits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CliFramework {
    Click,
    Typer,
    Cobra,
    Clap,
    Commander,
    Picocli,
    SystemCommandLine,
    SpectreCli,
    Argparse,
    Thor,
    Symfony,
    Laravel,
    /// DG.8: scopt (Scala), the builder's `programName` / `cmd` and scopt 3's
    /// `OptionParser[T]("x")`.
    Scopt,
    /// DH.10: Dart's package:args, a `CommandRunner('x', ..)` (the
    /// executable) and each `Command` subclass's `name`.
    CommandRunner,
    /// DH.10: an Elixir Mix task, `defmodule Mix.Tasks.Shop.Sync` with
    /// `use Mix.Task`, run as `mix shop.sync`.
    MixTask,
    /// LA.20c: a binary a packaging manifest declares (pyproject.toml
    /// `[project.scripts]`, setup.py `console_scripts`, package.json `bin`,
    /// Cargo.toml `[[bin]]`; DH.10: composer.json `bin`, a `*.gemspec`'s
    /// `executables`), not a subcommand a CLI library registers.
    Manifest,
}

impl CliFramework {
    /// The marker token for this framework.
    pub fn label(self) -> &'static str {
        match self {
            CliFramework::Click => "click",
            CliFramework::Typer => "typer",
            CliFramework::Cobra => "cobra",
            CliFramework::Clap => "clap",
            CliFramework::Commander => "commander",
            CliFramework::Picocli => "picocli",
            CliFramework::SystemCommandLine => "syscmd",
            CliFramework::SpectreCli => "spectre",
            CliFramework::Argparse => "argparse",
            CliFramework::Thor => "thor",
            CliFramework::Symfony => "symfony",
            CliFramework::Laravel => "laravel",
            CliFramework::Scopt => "scopt",
            CliFramework::CommandRunner => "commandrunner",
            CliFramework::MixTask => "mix",
            CliFramework::Manifest => "manifest",
        }
    }
}

/// What a per-file CLI extractor found. The invocation side fills `nodes` and
/// `nav` only.
#[derive(Default)]
pub struct CliNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    /// LA.20a: `cli:<name> --HANDLED_BY--> <implementation>` refs, bound by the
    /// graph builder's `resolve_refs` (its HANDLED_BY fallbacks leave an
    /// ambiguous name unresolved rather than guess).
    pub refs: Vec<UnresolvedRef>,
    /// LA.20a: distinct commands each framework declared in this file, sorted
    /// by framework. Feeds the `[cli-decl]` marker.
    pub per_framework: Vec<(CliFramework, usize)>,
    /// GD.21: what this file's residual forms read, for the `[cli] php ..`
    /// marker ([`Residuals::marker`]).
    residuals: Residuals,
}

/// GD.21: the residual CLI forms one file read, each counted by the scan
/// that read it: PHP's global `exec(` sites, fully qualified `new
/// \Symfony\Component\Process\Process(` sites, package:args `aliases`,
/// pubspec `executables:` keys and gemspec `executables.concat(..)` / `+=`
/// names. fired_on, once per file that read one:
/// `[cli] php exec=<e> symfony_fq=<s> args_aliases=<a> pubspec_exec=<p>
/// gemspec_concat=<g> path=<p>`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Residuals {
    php_exec: usize,
    symfony_fq: usize,
    args_aliases: usize,
    pubspec_exec: usize,
    gemspec_concat: usize,
}

impl Residuals {
    /// The marker line, or `None` when nothing was read.
    fn marker(self, path: &str) -> Option<String> {
        (self != Residuals::default()).then(|| {
            format!(
                "[cli] php exec={} symfony_fq={} args_aliases={} pubspec_exec={} gemspec_concat={} path={path}",
                self.php_exec,
                self.symfony_fq,
                self.args_aliases,
                self.pubspec_exec,
                self.gemspec_concat
            )
        })
    }
}

/// One command a scanner read: the name a user types, and the symbol that
/// implements it when the declaration says.
#[derive(Debug, PartialEq)]
struct CliDecl {
    framework: CliFramework,
    name: String,
    /// The code the command runs, with the 0-based row of the declaration
    /// that registers it (the annotation, decorator, builder call or
    /// `set_defaults`): the HANDLED_BY ref's site line (LC.3b).
    handler: Option<(CallQualifier, u32)>,
}

impl CliDecl {
    fn new(framework: CliFramework, name: String, handler: Option<(CallQualifier, u32)>) -> Self {
        CliDecl {
            framework,
            name,
            handler,
        }
    }
}

/// `handler` placed at the row of byte `at` of `source` (LC.3b).
fn at_row(handler: Option<CallQualifier>, source: &str, at: usize) -> Option<(CallQualifier, u32)> {
    handler.map(|h| (h, line_of(source, at)))
}

/// A line-at-a-time declaration reader.
type LineExtractor = fn(&str) -> Option<String>;

/// Every CLI_COMMAND a file declares, keyed flat as `cli:<name>` with the
/// command literal as the name, so A13.4's `CliInvocationResolver` pairs an
/// invocation's argv0 or subcommand word with it.
///
/// Dispatch is by `lang` (LA.20a): cobra `Use:` for `go`; commander `.command(`
/// for the TS family only (it minted phantoms from Ruby / Java `.command("x")`
/// calls before); clap for `rust` (the single-line `#[command(name` form, plus
/// the derive and builder scans in a file that mentions `clap`); picocli for
/// `java` and `kotlin`; System.CommandLine / Spectre.Console.Cli for `csharp`.
/// LA.20b: Typer / click decorators and argparse subparsers for `python`
/// (commander's needle no longer reads Python, so a non-decorator
/// `db.command("ping")` mints nothing); Thor for `ruby`; Symfony Console and
/// Laravel artisan for `php`. LA.20c: a `python` file's setuptools
/// `console_scripts` entry points (setup.py is Python source, so it is read
/// here, not by [`extract_manifest_binaries`]) declare binaries. DG.8: scopt
/// for `scala` (a file that mentions `scopt`). DH.10: package:args for `dart`
/// (a file that names `package:args/`), Mix tasks for `elixir` (a file that
/// names `Mix.Task`).
pub fn extract_cli_command_nodes(
    source: &str,
    lang: &str,
    module_id: NodeId,
    repo: RepoId,
) -> CliNodes {
    let mut decls: Vec<CliDecl> = Vec::new();
    let mut residuals = Residuals::default();

    let line_extractors: Vec<(LineExtractor, CliFramework)> = match lang {
        "go" => vec![(
            extract_cobra_command_name as LineExtractor,
            CliFramework::Cobra,
        )],
        "typescript" | "react" | "angular" | "vue" => {
            vec![(
                extract_commander_command_name as LineExtractor,
                CliFramework::Commander,
            )]
        }
        "rust" => vec![(
            extract_clap_command_name as LineExtractor,
            CliFramework::Clap,
        )],
        _ => Vec::new(),
    };
    if !line_extractors.is_empty() {
        let mut seen = std::collections::HashSet::new();
        for line in source.lines() {
            let trimmed = line.trim();
            for (extractor, framework) in &line_extractors {
                if let Some(name) = extractor(trimmed)
                    && seen.insert(name.clone())
                {
                    decls.push(CliDecl::new(*framework, name, None));
                    break;
                }
            }
        }
    }

    match lang {
        "rust" if source.contains("clap") => {
            let code = CodeMap::new(source);
            decls.extend(scan_clap_derive(source, &code));
            decls.extend(scan_clap_builder(source, &code));
        }
        "java" | "kotlin" if source.contains("picocli") => {
            decls.extend(scan_picocli(source, &CodeMap::new(source)));
        }
        "csharp"
            if source.contains("System.CommandLine") || source.contains("Spectre.Console.Cli") =>
        {
            decls.extend(scan_dotnet_cli(source, &CodeMap::new(source)));
        }
        "python"
            if source.contains(".command")
                || source.contains("add_typer")
                || source.contains("argparse") =>
        {
            let code = CodeMap::script(source, Script::Python);
            decls.extend(scan_typer_click(source, &code));
            decls.extend(scan_argparse(source, &code));
        }
        "ruby" if source.contains("Thor") => {
            decls.extend(scan_thor(source, &CodeMap::script(source, Script::Ruby)));
        }
        "php" if source.contains("Command") => {
            decls.extend(scan_symfony_laravel(
                source,
                &CodeMap::script(source, Script::Php),
            ));
        }
        "scala" if source.contains("scopt") => {
            decls.extend(scan_scopt(source, &CodeMap::new(source)));
        }
        "dart" if source.contains("package:args/") => {
            let (dart, aliases) = scan_dart_args(source, &CodeMap::script(source, Script::Dart));
            decls.extend(dart);
            residuals.args_aliases = aliases;
        }
        "elixir" if source.contains("Mix.Task") => {
            decls.extend(scan_mix_tasks(
                source,
                &CodeMap::script(source, Script::Elixir),
            ));
        }
        _ => {}
    }

    let setup_bins = if lang == "python" && source.contains("console_scripts") {
        scan_setup_py_console_scripts(source)
    } else {
        Vec::new()
    };
    decls.extend(
        setup_bins
            .iter()
            .map(|bin| CliDecl::new(CliFramework::Manifest, bin.name.clone(), None)),
    );

    let mut out = CliNodes::default();
    let mut minted: std::collections::HashMap<String, NodeId> = std::collections::HashMap::new();
    let mut handled: Vec<(NodeId, CallQualifier, u32)> = Vec::new();
    for decl in decls {
        let id = match minted.get(&decl.name) {
            Some(&id) => id,
            None => {
                let id = add_cli_command(&mut out.nodes, &mut out.nav, &decl.name, module_id, repo);
                minted.insert(decl.name.clone(), id);
                match out
                    .per_framework
                    .iter_mut()
                    .find(|(f, _)| *f == decl.framework)
                {
                    Some((_, n)) => *n += 1,
                    None => out.per_framework.push((decl.framework, 1)),
                }
                id
            }
        };
        if let Some((handler, line)) = decl.handler {
            handled.push((id, handler, line));
        }
    }
    let mut bound: Vec<(NodeId, CallQualifier)> = Vec::new();
    for (id, handler, line) in handled {
        // LA.20b stopgap: a CLI_COMMAND is a child of its file's MODULE, and
        // `build_symbol_table` (graph/src/build.rs) indexes every module child
        // by name, the command last. A `Bare(h)` handler with `h` equal to a
        // command minted in this file (`@click.command("sync") def sync`)
        // would bind to that command, a self-loop, so it is not emitted.
        // Remove once `build_symbol_table` stops indexing CLI_COMMAND children
        // into `module_symbols`.
        let shadowed = matches!(&handler, CallQualifier::Bare(h) if minted.contains_key(h));
        if !shadowed && !bound.iter().any(|(from, q)| *from == id && *q == handler) {
            bound.push((id, handler.clone()));
            out.refs.push(UnresolvedRef {
                from: id,
                from_module: module_id,
                qualifier: handler,
                category: edge_category::HANDLED_BY,
                line,
            });
        }
    }
    // GH.20: a setup.py binary carries its target for resolver:host (no
    // `file`: its parent MODULE is the located setup.py). The first entry of
    // a name wins, as it minted the command.
    for bin in &setup_bins {
        let (Some(target), Some(&id)) = (bin.target.as_deref(), minted.get(&bin.name)) else {
            continue;
        };
        if let Some(node) = out.nodes.iter_mut().find(|n| n.id == id)
            && !node
                .cells
                .iter()
                .any(|c| parse_manifest_bin_cell(c).is_some())
        {
            node.cells.push(manifest_bin_cell(
                ManifestForm::SetupPy.label(),
                None,
                bin.line,
                target,
            ));
        }
    }
    out.per_framework.sort();
    out.residuals = residuals;
    out
}

/// LA.20a fired_on: `[cli-decl] rust=clap:3 handler_refs=0 path=src/main.rs`,
/// or `None` when the file declared no command. Only frameworks that fired are
/// listed; the prefix and `path=` are stable. LA.20c: a python file that
/// declares only setuptools `console_scripts` binaries (a setup.py) prints the
/// manifest line instead, `[cli-decl] manifest bins=1 (pyproject=0 setup=1
/// npm=0 cargo=0 composer=0 gemspec=0) path=setup.py`, the one every
/// manifest-declared binary shares. GD.21: a Dart file whose commands
/// declared package:args `aliases` gets a second line, `[cli] php exec=0
/// symfony_fq=0 args_aliases=<a> pubspec_exec=0 gemspec_concat=0 path=<p>`,
/// joined by `\n` (the caller prints the whole value to stderr).
pub fn decl_marker(lang: &str, out: &CliNodes, path: &str) -> Option<String> {
    if out.per_framework.is_empty() {
        return None;
    }
    if lang == "python"
        && let [(CliFramework::Manifest, n)] = out.per_framework.as_slice()
    {
        return Some(manifest_line(ManifestForm::SetupPy, *n, path));
    }
    let counts: Vec<String> = out
        .per_framework
        .iter()
        .map(|(f, n)| format!("{}:{n}", f.label()))
        .collect();
    let decl = format!(
        "[cli-decl] {lang}={} handler_refs={} path={path}",
        counts.join(","),
        out.refs.len()
    );
    Some(match out.residuals.marker(path) {
        Some(residual) => format!("{decl}\n{residual}"),
        None => decl,
    })
}

fn add_cli_command(
    nodes: &mut Vec<Node>,
    nav: &mut CodeNav,
    name: &str,
    module_id: NodeId,
    repo: RepoId,
) -> NodeId {
    let qname = format!("cli:{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLI_COMMAND, &qname);
    nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: vec![],
    });
    nav.record(id, name, &qname, node_kind::CLI_COMMAND, Some(module_id));
    id
}

fn extract_cobra_command_name(line: &str) -> Option<String> {
    if !line.contains("cobra.Command") {
        return None;
    }
    let use_idx = line.find("Use:")?;
    let after = &line[use_idx + 4..];
    extract_quoted(after.trim_start())
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

// ----------------------------------------------------------------------------
// LA.20a: bracket-aware scanning shared by the attribute / annotation scanners.
// Every delimiter is ASCII, so a byte index that sits on one (or just past it)
// is a char boundary; every slice below is taken only at such an index.
// ----------------------------------------------------------------------------

/// Longest attribute / annotation argument list [`attr_span`] reads.
const MAX_ATTR_SPAN: usize = 2048;
/// Longest enum body the clap Subcommand scan reads.
const MAX_ENUM_BODY: usize = 64 * 1024;
/// Longest `use ...;` statement the clap builder gate reads.
const MAX_USE_STMT: usize = 1024;

/// The text strictly inside the balanced `(`...`)` (or `[`...`]`, `{`...`}`)
/// group that opens at byte `open_at`, reading at most [`MAX_ATTR_SPAN`] bytes.
/// String literals, char literals and comments are skipped, so a `)` or `"`
/// inside them does not end the span. `None` when `open_at` is not an opening
/// bracket, or the group does not close within the cap.
fn attr_span(source: &str, open_at: usize) -> Option<&str> {
    balanced(source, open_at, MAX_ATTR_SPAN).map(|(start, end)| &source[start..end])
}

/// `(inner_start, inner_end)` of the bracket group opening at `open_at`. All
/// three bracket kinds nest; a mismatched closer is `None`.
fn balanced(source: &str, open_at: usize, cap: usize) -> Option<(usize, usize)> {
    let b = source.as_bytes();
    let first = *b.get(open_at)?;
    if !matches!(first, b'(' | b'[' | b'{') {
        return None;
    }
    let closer = |c: u8| match c {
        b'(' => b')',
        b'[' => b']',
        _ => b'}',
    };
    let limit = b.len().min(open_at.saturating_add(cap));
    let mut stack = vec![closer(first)];
    let mut i = open_at + 1;
    while i < limit {
        match b[i] {
            b'"' => {
                i = skip_string(b, i, limit)?;
                continue;
            }
            b'\'' => {
                if let Some(end) = char_literal_end(b, i) {
                    i = end;
                    continue;
                }
            }
            b'r' if i == 0 || !is_ident_byte(b[i - 1]) => {
                if let Some(end) = skip_raw_string(b, i) {
                    i = end;
                    continue;
                }
            }
            b'/' if b.get(i + 1).is_some_and(|&c| c == b'/' || c == b'*') => {
                i = skip_comment(b, i);
                continue;
            }
            c @ (b'(' | b'[' | b'{') => stack.push(closer(c)),
            c @ (b')' | b']' | b'}') => {
                if stack.pop()? != c {
                    return None;
                }
                if stack.is_empty() {
                    return Some((open_at + 1, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Index just past the `"`-string starting at `at` (`\` escapes honoured), or
/// `None` when it does not close before `limit`.
fn skip_string(b: &[u8], at: usize, limit: usize) -> Option<usize> {
    let mut j = at + 1;
    while j < limit {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// Index just past a char literal (`'x'`, `'é'`, `'\n'`, `'\''`, `'\u{1F600}'`)
/// starting at `at`, or `None` when the `'` is something else (a Rust
/// lifetime, an apostrophe).
fn char_literal_end(b: &[u8], at: usize) -> Option<usize> {
    let close_within = |from: usize, to: usize| {
        (from..to.min(b.len()))
            .find(|&j| b[j] == b'\'')
            .map(|j| j + 1)
    };
    match *b.get(at + 1)? {
        b'\\' => close_within(at + 3, at + 12),
        b'\'' => None,
        c if c.is_ascii() => (b.get(at + 2) == Some(&b'\'')).then_some(at + 3),
        // One multi-byte char: only non-ASCII bytes up to the closing quote.
        _ => close_within(at + 2, at + 6)
            .filter(|&end| b[at + 1..end - 1].iter().all(|c| !c.is_ascii())),
    }
}

/// Where a scanner may match: every byte outside string literals, char
/// literals and comments. Built once per file by one lexing pass over
/// `"..."` (with `\` escapes; also Rust `b"..."`), Rust raw strings
/// (`r"..."`, `r#"..."#`, `br#"..."#`), Java / C# `"""..."""` blocks, C#
/// verbatim strings (`@"..."`, `""` escapes), char literals, `//` comments and
/// nesting `/* */` comments. A needle in a doc comment or a string (a test
/// fixture, a scanner's own needle table) is not a declaration.
/// [`crate::code_guard`] is the second user: it guards the queue and event
/// needle scanners in Rust and Python files by the same rule (CJ.1a).
pub(crate) struct CodeMap {
    /// `[start, end)` of each literal / comment, in source order.
    skips: Vec<(usize, usize)>,
}

impl CodeMap {
    pub(crate) fn new(source: &str) -> Self {
        let b = source.as_bytes();
        let mut skips = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let prev = |k: usize| i.checked_sub(k).map(|p| b[p]);
            let end = match b[i] {
                b'/' if b.get(i + 1) == Some(&b'/') => Some(skip_comment(b, i)),
                b'/' if b.get(i + 1) == Some(&b'*') => Some(skip_nested_block_comment(b, i)),
                b'"' if b[i..].starts_with(b"\"\"\"") => {
                    Some(find_bytes(b, i + 3, b"\"\"\"").map_or(b.len(), |j| j + 3))
                }
                b'"' if prev(1) == Some(b'@')
                    || (prev(1) == Some(b'$') && prev(2) == Some(b'@')) =>
                {
                    Some(skip_verbatim_string(b, i))
                }
                b'"' => Some(skip_string(b, i, b.len()).unwrap_or(b.len())),
                b'r' if !prev(1).is_some_and(is_ident_byte)
                    || (prev(1) == Some(b'b') && !prev(2).is_some_and(is_ident_byte)) =>
                {
                    skip_raw_string(b, i)
                }
                b'\'' => char_literal_end(b, i),
                _ => None,
            };
            match end {
                Some(end) if end > i => {
                    skips.push((i, end));
                    i = end;
                }
                _ => i += 1,
            }
        }
        CodeMap { skips }
    }

    /// True when byte `pos` is code, not inside a literal or comment.
    pub(crate) fn is_code(&self, pos: usize) -> bool {
        let i = self.skips.partition_point(|&(start, _)| start <= pos);
        i == 0 || self.skips[i - 1].1 <= pos
    }

    /// CJ.1a: the first byte of the literal or comment holding `pos` (its
    /// opening quote, its `//` / `#`), or `None` when `pos` is code.
    pub(crate) fn literal_start(&self, pos: usize) -> Option<usize> {
        let i = self.skips.partition_point(|&(start, _)| start <= pos);
        let (start, end) = *self.skips.get(i.checked_sub(1)?)?;
        (pos < end).then_some(start)
    }
}

/// First index at or after `from` where `needle` starts.
fn find_bytes(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    b.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

/// Index just past the (Rust-nesting) `/* */` comment at `at`; the end of
/// input when it never closes.
fn skip_nested_block_comment(b: &[u8], at: usize) -> usize {
    let (mut depth, mut j) = (0usize, at);
    while j + 1 < b.len() {
        match (b[j], b[j + 1]) {
            (b'/', b'*') => {
                depth += 1;
                j += 2;
            }
            (b'*', b'/') => {
                depth = depth.saturating_sub(1);
                j += 2;
                if depth == 0 {
                    return j;
                }
            }
            _ => j += 1,
        }
    }
    b.len()
}

/// Index just past the C# verbatim string whose opening `"` is at `at` (`""`
/// is an escaped quote, `\` is literal).
fn skip_verbatim_string(b: &[u8], at: usize) -> usize {
    let mut j = at + 1;
    while j < b.len() {
        if b[j] == b'"' {
            if b.get(j + 1) == Some(&b'"') {
                j += 2;
                continue;
            }
            return j + 1;
        }
        j += 1;
    }
    b.len()
}

/// Index just past the Rust raw string whose `r` is at `at` (`r"..."`,
/// `r##"..."##`), or `None` when the `r` does not open one (`r#ident`, `for`).
fn skip_raw_string(b: &[u8], at: usize) -> Option<usize> {
    let mut j = at + 1;
    while b.get(j) == Some(&b'#') {
        j += 1;
    }
    if b.get(j) != Some(&b'"') {
        return None;
    }
    let hashes = j - at - 1;
    let mut k = j + 1;
    while k < b.len() {
        if b[k] == b'"'
            && b.get(k + 1..k + 1 + hashes)
                .is_some_and(|h| h.iter().all(|&c| c == b'#'))
        {
            return Some(k + 1 + hashes);
        }
        k += 1;
    }
    Some(b.len())
}

/// Index just past the `//` line comment or `/* */` block comment at `at` (the
/// end of input when a block comment never closes).
fn skip_comment(b: &[u8], at: usize) -> usize {
    if b.get(at + 1) == Some(&b'*') {
        let mut j = at + 2;
        while j + 1 < b.len() {
            if b[j] == b'*' && b[j + 1] == b'/' {
                return j + 2;
            }
            j += 1;
        }
        b.len()
    } else {
        let mut j = at;
        while j < b.len() && b[j] != b'\n' {
            j += 1;
        }
        j
    }
}

/// The first index at or after `i` that is neither whitespace nor inside a
/// comment.
fn skip_trivia(b: &[u8], mut i: usize) -> usize {
    loop {
        while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
            i += 1;
        }
        if b.get(i) == Some(&b'/') && b.get(i + 1).is_some_and(|&c| c == b'/' || c == b'*') {
            i = skip_comment(b, i);
        } else {
            return i;
        }
    }
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// The ASCII identifier starting at `i` (empty when there is none) and the
/// index after it.
fn read_ident(source: &str, i: usize) -> (&str, usize) {
    let b = source.as_bytes();
    let mut j = i;
    if b.get(j)
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
    {
        while b.get(j).copied().is_some_and(is_ident_byte) {
            j += 1;
        }
    }
    (source.get(i..j).unwrap_or(""), j)
}

/// Byte offsets where `word` occurs as a whole word outside the string
/// literals and comments of `text`: no identifier byte or `.` before it, no
/// identifier byte after it.
fn word_positions(text: &str, word: &str) -> Vec<usize> {
    let (b, w) = (text.as_bytes(), word.as_bytes());
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i = skip_string(b, i, b.len()).unwrap_or(b.len());
                continue;
            }
            b'/' if b.get(i + 1).is_some_and(|&c| c == b'/' || c == b'*') => {
                i = skip_comment(b, i);
                continue;
            }
            _ => {}
        }
        if b[i..].starts_with(w)
            && (i == 0 || !(is_ident_byte(b[i - 1]) || b[i - 1] == b'.'))
            && !b.get(i + w.len()).copied().is_some_and(is_ident_byte)
        {
            out.push(i);
            i += w.len();
            continue;
        }
        i += 1;
    }
    out
}

/// The string value of `key = "value"` inside an attribute / annotation span
/// (`name = "shopctl"`). A key inside a string literal does not count.
fn attr_kv(span: &str, key: &str) -> Option<String> {
    let b = span.as_bytes();
    word_positions(span, key).into_iter().find_map(|at| {
        let i = skip_trivia(b, at + key.len());
        if b.get(i) != Some(&b'=') || b.get(i + 1) == Some(&b'=') {
            return None;
        }
        let v = skip_trivia(b, i + 1);
        if b.get(v) != Some(&b'"') {
            return None;
        }
        extract_quoted(span.get(v..)?)
    })
}

/// True when `flag` stands alone in an attribute span (`flatten`,
/// `external_subcommand`): a whole word followed by neither `=` nor `(`.
fn attr_flag(span: &str, flag: &str) -> bool {
    let b = span.as_bytes();
    word_positions(span, flag).into_iter().any(|at| {
        let i = skip_trivia(b, at + flag.len());
        !matches!(b.get(i), Some(b'=' | b'('))
    })
}

/// The body of each `use ...;` statement in `source` (text after `use`, up to
/// the `;`), skipping any longer than [`MAX_USE_STMT`].
fn use_statements<'a>(source: &'a str, code: &CodeMap) -> Vec<&'a str> {
    let b = source.as_bytes();
    word_positions(source, "use")
        .into_iter()
        .filter(|&at| code.is_code(at))
        .filter_map(|at| {
            let start = skip_trivia(b, at + 3);
            let rest = source.get(start..)?;
            let end = rest.find(';').filter(|&e| e <= MAX_USE_STMT)?;
            Some(&rest[..end])
        })
        .collect()
}

// ----------------------------------------------------------------------------
// LA.20a: clap (rust)
// ----------------------------------------------------------------------------

/// One `#[path(args)]` / `#[path]` attribute.
struct RustAttr<'a> {
    /// Last path segment: `command`, `clap`, `derive`, `arg`.
    name: &'a str,
    args: Option<&'a str>,
    /// Index just past the closing `]`.
    end: usize,
}

/// The attribute whose `#` is at `at`, or `None` when `source[at..]` is not
/// `#[` or the attribute does not close within [`MAX_ATTR_SPAN`].
fn read_rust_attr(source: &str, at: usize) -> Option<RustAttr<'_>> {
    let b = source.as_bytes();
    if b.get(at) != Some(&b'#') || b.get(at + 1) != Some(&b'[') {
        return None;
    }
    let (_, close) = balanced(source, at + 1, MAX_ATTR_SPAN)?;
    let mut i = skip_trivia(b, at + 2);
    let mut name = "";
    loop {
        let (seg, j) = read_ident(source, i);
        if seg.is_empty() {
            break;
        }
        name = seg;
        i = j;
        if b.get(i) == Some(&b':') && b.get(i + 1) == Some(&b':') {
            i += 2;
        } else {
            break;
        }
    }
    let i = skip_trivia(b, i);
    let args = if i < close && b.get(i) == Some(&b'(') {
        attr_span(source, i)
    } else {
        None
    };
    Some(RustAttr {
        name,
        args,
        end: close + 1,
    })
}

/// The run of attributes starting at `at` (comments between them allowed) and
/// the index of the item they decorate.
fn read_attr_run(source: &str, at: usize) -> (Vec<RustAttr<'_>>, usize) {
    let b = source.as_bytes();
    let mut attrs = Vec::new();
    let mut i = at;
    while let Some(attr) = read_rust_attr(source, i) {
        i = skip_trivia(b, attr.end);
        attrs.push(attr);
    }
    (attrs, i)
}

/// The keyword of the Rust item at `i`, past any visibility (`pub`,
/// `pub(crate)`), and the index after it.
fn rust_item_keyword(source: &str, i: usize) -> (&str, usize) {
    let b = source.as_bytes();
    let (word, j) = read_ident(source, i);
    if word != "pub" {
        return (word, j);
    }
    let mut k = skip_trivia(b, j);
    if b.get(k) == Some(&b'(')
        && let Some((_, close)) = balanced(source, k, MAX_ATTR_SPAN)
    {
        k = skip_trivia(b, close + 1);
    }
    read_ident(source, k)
}

/// The argument span of a clap command attribute (`#[command(...)]`, and
/// clap 2 / 3's `#[clap(...)]`).
fn clap_command_args<'a>(attr: &RustAttr<'a>) -> Option<&'a str> {
    if matches!(attr.name, "command" | "clap") {
        attr.args
    } else {
        None
    }
}

/// True for a `#[derive(...)]` that lists `Subcommand` or `Parser` (on an
/// enum, both make each variant a command).
fn derives_subcommand(attr: &RustAttr<'_>) -> bool {
    attr.name == "derive"
        && attr.args.is_some_and(|d| {
            d.split(',')
                .filter_map(|t| t.trim().rsplit("::").next())
                .any(|t| t == "Subcommand" || t == "Parser")
        })
}

/// clap derive: every `name = "x"` on a `#[command(...)]` / `#[clap(...)]`
/// that decorates a struct or enum (multi-line spans included), and every
/// variant of a `#[derive(Subcommand)]` enum (or a `#[derive(Parser)]` enum,
/// whose variants are top-level subcommands). A variant is named by its own
/// `name = "x"`, else by the enum's `rename_all` rule, else by clap's default
/// kebab-case. `flatten` / `external_subcommand` / `skip` variants name no
/// command, and a field attribute such as `#[command(subcommand)]` never does.
fn scan_clap_derive(source: &str, code: &CodeMap) -> Vec<CliDecl> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = source.get(from..).and_then(|s| s.find("#[")) {
        let at = from + rel;
        if !code.is_code(at) {
            from = at + 2;
            continue;
        }
        let (attrs, item_at) = read_attr_run(source, at);
        from = item_at.max(at + 2);
        let (keyword, after_kw) = rust_item_keyword(source, item_at);
        if attrs.is_empty() || !matches!(keyword, "struct" | "enum") {
            continue;
        }
        for args in attrs.iter().filter_map(clap_command_args) {
            if let Some(name) = attr_kv(args, "name") {
                out.push(CliDecl::new(CliFramework::Clap, name, None));
            }
        }
        if keyword != "enum" || !attrs.iter().any(derives_subcommand) {
            continue;
        }
        let rule = attrs
            .iter()
            .filter_map(clap_command_args)
            .find_map(|a| attr_kv(a, "rename_all"));
        let Some(open) = source
            .get(after_kw..)
            .and_then(|s| s.find('{'))
            .map(|r| after_kw + r)
        else {
            continue;
        };
        let Some((body_start, body_end)) = balanced(source, open, MAX_ENUM_BODY) else {
            continue;
        };
        for (variant, vattrs) in enum_variants(source, body_start, body_end) {
            let vargs: Vec<&str> = vattrs.iter().filter_map(clap_command_args).collect();
            let unnamed = |a: &&str| {
                attr_flag(a, "flatten")
                    || attr_flag(a, "external_subcommand")
                    || attr_flag(a, "skip")
            };
            if vargs.iter().any(unnamed) {
                continue;
            }
            let name = vargs
                .iter()
                .find_map(|a| attr_kv(a, "name"))
                .unwrap_or_else(|| clap_rename(variant, rule.as_deref()));
            if !name.is_empty() {
                out.push(CliDecl::new(CliFramework::Clap, name, None));
            }
        }
        from = from.max(body_end);
    }
    out
}

/// The variants of the enum body `source[start..end]`, each with its
/// attributes: `Name`, `Name { .. }`, `Name(..)`, `Name = 3`, separated by
/// depth-0 commas, comments skipped.
fn enum_variants(source: &str, start: usize, end: usize) -> Vec<(&str, Vec<RustAttr<'_>>)> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        let (attrs, at) = read_attr_run(source, skip_trivia(b, i));
        let (name, after) = read_ident(source, at);
        if name.is_empty() || after > end {
            break;
        }
        let mut j = skip_trivia(b, after);
        if matches!(b.get(j), Some(b'{' | b'(')) {
            let Some((_, close)) = balanced(source, j, MAX_ENUM_BODY) else {
                break;
            };
            j = skip_trivia(b, close + 1);
        }
        if b.get(j) == Some(&b'=') {
            while j < end && b[j] != b',' {
                j += 1;
            }
        }
        out.push((name, attrs));
        if j < end && b[j] == b',' {
            i = j + 1;
        } else {
            break;
        }
    }
    out
}

/// The words of an identifier, split the way `heck` (and so clap) splits them:
/// at `_`, at a lower-to-upper step, and before the last capital of an acronym
/// run that a lowercase letter follows (`HTTPServe` -> `HTTP`, `Serve`).
/// Digits stay with the word they follow (`V2Api` -> `V2`, `Api`).
fn heck_words(ident: &str) -> Vec<&str> {
    #[derive(PartialEq, Clone, Copy)]
    enum Mode {
        Boundary,
        Lower,
        Upper,
    }
    let mut words = Vec::new();
    for part in ident
        .split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
    {
        let mut chars = part.char_indices().peekable();
        let mut init = 0;
        let mut mode = Mode::Boundary;
        while let Some((i, c)) = chars.next() {
            let Some(&(next_i, next)) = chars.peek() else {
                words.push(&part[init..]);
                break;
            };
            let next_mode = if c.is_lowercase() {
                Mode::Lower
            } else if c.is_uppercase() {
                Mode::Upper
            } else {
                mode
            };
            if next_mode == Mode::Lower && next.is_uppercase() {
                words.push(&part[init..next_i]);
                init = next_i;
                mode = Mode::Boundary;
            } else if mode == Mode::Upper && c.is_uppercase() && next.is_lowercase() {
                words.push(&part[init..i]);
                init = i;
                mode = Mode::Boundary;
            } else {
                mode = next_mode;
            }
        }
    }
    words
}

/// `Word` from `wORD`: first char upper, the rest lower.
fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// A variant's command name under clap's `rename_all` rule (default
/// kebab-case). The rule string is normalised the way clap_derive does it, so
/// `snake_case`, `snake` and `SnakeCase` are one rule; an unknown rule falls
/// back to the default.
fn clap_rename(variant: &str, rule: Option<&str>) -> String {
    let words = heck_words(variant);
    let lower = |sep: &str| {
        words
            .iter()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join(sep)
    };
    let upper = |sep: &str| {
        words
            .iter()
            .map(|w| w.to_uppercase())
            .collect::<Vec<_>>()
            .join(sep)
    };
    let rule: String = rule
        .unwrap_or("kebab")
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    match rule.strip_suffix("case").unwrap_or(&rule) {
        "snake" => lower("_"),
        "lower" => lower(""),
        "upper" => upper(""),
        "screamingsnake" => upper("_"),
        "verbatim" => variant.to_string(),
        "pascal" => words.iter().map(|w| capitalise(w)).collect(),
        "camel" => words
            .iter()
            .enumerate()
            .map(|(i, w)| {
                if i == 0 {
                    w.to_lowercase()
                } else {
                    capitalise(w)
                }
            })
            .collect(),
        _ => lower("-"),
    }
}

/// clap builder: `Command::new("x")` (clap 3 / 4), `App::new("x")` and
/// `SubCommand::with_name("x")` (clap 2 / 3) -> `cli:x`. A bare
/// `Command::new` counts only when a `use clap::...` lists `Command` and no
/// `use ...process::...` brings in the std / tokio `Command` of the same name
/// (that one is an invocation); `clap::Command::new` always counts.
fn scan_clap_builder(source: &str, code: &CodeMap) -> Vec<CliDecl> {
    let uses = use_statements(source, code);
    let lists = |prefix: &str, word: &str| {
        uses.iter().any(|u| {
            u.starts_with(prefix) && (!word_positions(u, word).is_empty() || u.contains('*'))
        })
    };
    let process_command = uses
        .iter()
        .any(|u| u.contains("process::") && !word_positions(u, "Command").is_empty());
    let bare_command = lists("clap::", "Command") && !process_command;
    let bare_app = lists("clap::", "App");
    let mut out = Vec::new();
    for (needle, bare_ok) in [
        ("Command::new(", bare_command),
        ("App::new(", bare_app),
        ("SubCommand::with_name(", true),
    ] {
        let mut from = 0;
        while let Some(rel) = source.get(from..).and_then(|s| s.find(needle)) {
            let at = from + rel;
            from = at + needle.len();
            let prefix = &source[..at];
            let accepted = if !code.is_code(at) {
                false
            } else if prefix.ends_with("::") {
                prefix.ends_with("clap::") || prefix.ends_with("clap::builder::")
            } else {
                bare_ok
                    && !prefix
                        .bytes()
                        .last()
                        .is_some_and(|c| is_ident_byte(c) || c == b'.')
            };
            if accepted && let Some(name) = extract_quoted(source[from..].trim_start()) {
                out.push(CliDecl::new(CliFramework::Clap, name, None));
            }
        }
    }
    out
}

// ----------------------------------------------------------------------------
// LA.20a: picocli (java / kotlin)
// ----------------------------------------------------------------------------

/// picocli: every `@Command(...)` / `@CommandLine.Command(...)` whose span
/// carries `name = "x"` -> `cli:x`. The handler is the class the annotation
/// decorates (`Bare(Class)`); on a method it is that method, as
/// `Attribute { enclosing class, method }` (methods are not module symbols, so
/// it binds through the unique-method fallback), or `Bare(fun)` for a Kotlin
/// top-level function.
fn scan_picocli(source: &str, code: &CodeMap) -> Vec<CliDecl> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = source.get(from..).and_then(|s| s.find('@')) {
        let at = from + rel;
        from = at + 1;
        if !code.is_code(at) {
            continue;
        }
        let (name_end, last) = read_dotted(source, at + 1);
        if last != "Command" {
            continue;
        }
        let open = skip_trivia(b, name_end);
        let Some(span) = attr_span(source, open) else {
            continue;
        };
        let close = open + 1 + span.len();
        from = close;
        let Some(name) = attr_kv(span, "name") else {
            continue;
        };
        let handler = picocli_handler(source, code, close + 1, at);
        out.push(CliDecl::new(
            CliFramework::Picocli,
            name,
            at_row(handler, source, at),
        ));
    }
    out
}

/// The dotted name starting at `i` (`CommandLine.Command`): the index after it
/// and its last segment.
fn read_dotted(source: &str, i: usize) -> (usize, &str) {
    let b = source.as_bytes();
    let (mut last, mut j) = read_ident(source, i);
    while !last.is_empty() && b.get(j) == Some(&b'.') {
        let (seg, k) = read_ident(source, j + 1);
        if seg.is_empty() {
            break;
        }
        (last, j) = (seg, k);
    }
    (j, last)
}

/// What the annotation ending at `end` decorates: past further annotations and
/// modifiers, a `class` / `record` / `object` -> `Bare(Name)`, or a method (the
/// identifier before its `(`) -> `Attribute { enclosing class, method }`.
fn picocli_handler(
    source: &str,
    code: &CodeMap,
    end: usize,
    annotation_at: usize,
) -> Option<CallQualifier> {
    let b = source.as_bytes();
    let mut i = skip_trivia(b, end);
    while b.get(i) == Some(&b'@') {
        let (after, _) = read_dotted(source, i + 1);
        i = skip_trivia(b, after);
        if b.get(i) == Some(&b'(') {
            let (_, close) = balanced(source, i, MAX_ATTR_SPAN)?;
            i = skip_trivia(b, close + 1);
        }
    }
    // Modifiers, a return type and the name: a handful of words at most.
    for _ in 0..16 {
        let (word, after) = read_ident(source, i);
        if word.is_empty() {
            return None;
        }
        if matches!(word, "class" | "record" | "object") {
            let (name, _) = read_ident(source, skip_trivia(b, after));
            return (!name.is_empty()).then(|| CallQualifier::Bare(name.to_string()));
        }
        i = skip_trivia(b, after);
        if b.get(i) == Some(&b'(') {
            let method = word.to_string();
            return Some(match enclosing_class(source, code, annotation_at) {
                Some(class) => CallQualifier::Attribute {
                    base: class.to_string(),
                    name: method,
                },
                None => CallQualifier::Bare(method),
            });
        }
        // Generic / array / nullable return types: `List<String> run(`.
        while matches!(
            b.get(i),
            Some(b'<' | b'>' | b'[' | b']' | b',' | b'?' | b'.')
        ) {
            i = skip_trivia(b, i + 1);
        }
    }
    None
}

/// The class whose declaration most recently precedes `at` (`class Tool`).
fn enclosing_class<'a>(source: &'a str, code: &CodeMap, at: usize) -> Option<&'a str> {
    let head = source.get(..at)?;
    word_positions(head, "class")
        .into_iter()
        .rev()
        .filter(|&p| code.is_code(p))
        .find_map(|p| {
            let (name, _) = read_ident(source, skip_trivia(source.as_bytes(), p + 5));
            (!name.is_empty()).then_some(name)
        })
}

// ----------------------------------------------------------------------------
// LA.20a: System.CommandLine / Spectre.Console.Cli (csharp)
// ----------------------------------------------------------------------------

/// .NET: System.CommandLine (in a file that mentions `System.CommandLine`)
/// and Spectre.Console.Cli (in one that mentions `Spectre.Console.Cli`).
fn scan_dotnet_cli(source: &str, code: &CodeMap) -> Vec<CliDecl> {
    let mut out = Vec::new();
    if source.contains("System.CommandLine") {
        scan_system_commandline(source, code, &mut out);
    }
    if source.contains("Spectre.Console.Cli") {
        scan_spectre_cli(source, code, &mut out);
    }
    out
}

/// `new Command("x", ...)` -> `cli:x`; when it is assigned to a variable `v`,
/// `v.SetHandler(H` / `v.SetAction(H` with a method group `H` (`Name` or
/// `Type.Name`) is its handler. `new RootCommand("...")` carries a
/// description, not a name, so it declares nothing.
fn scan_system_commandline(source: &str, code: &CodeMap, out: &mut Vec<CliDecl>) {
    let b = source.as_bytes();
    for at in word_positions(source, "new")
        .into_iter()
        .filter(|&at| code.is_code(at))
    {
        let (ty, after) = read_ident(source, skip_trivia(b, at + 3));
        if ty != "Command" {
            continue;
        }
        let open = skip_trivia(b, after);
        if b.get(open) != Some(&b'(') {
            continue;
        }
        let mut first = skip_trivia(b, open + 1);
        if source.get(first..).is_some_and(|s| s.starts_with("name:")) {
            first = skip_trivia(b, first + 5);
        }
        let Some(name) = source.get(first..).and_then(extract_quoted) else {
            continue;
        };
        let handler =
            assigned_variable(source, at).and_then(|v| set_handler_target(source, code, v));
        out.push(CliDecl::new(
            CliFramework::SystemCommandLine,
            name,
            at_row(handler, source, at),
        ));
    }
}

/// The variable the expression at `at` is assigned to: `var sync = ` /
/// `Command sync = ` -> `sync`. Not for `==`, `+=`, `=>` and friends.
fn assigned_variable(source: &str, at: usize) -> Option<&str> {
    let head = source.get(..at)?.trim_end().strip_suffix('=')?;
    if head.ends_with([
        '=', '!', '<', '>', '+', '-', '*', '/', '%', '&', '|', '^', '?',
    ]) {
        return None;
    }
    let head = head.trim_end();
    let start = head
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '_'))
        .map_or(0, |(p, c)| p + c.len_utf8());
    let var = &head[start..];
    (!var.is_empty() && !var.starts_with(|c: char| c.is_ascii_digit())).then_some(var)
}

/// The method group passed to `var.SetHandler(` / `var.SetAction(`: `Bare(H)`
/// for `H`, `Attribute { T, H }` for `T.H`. A lambda or a call is not a
/// handler.
fn set_handler_target(source: &str, code: &CodeMap, var: &str) -> Option<CallQualifier> {
    let b = source.as_bytes();
    word_positions(source, var)
        .into_iter()
        .filter(|&at| code.is_code(at))
        .find_map(|at| {
            let after_var = at + var.len();
            let rest = source.get(after_var..)?.strip_prefix('.')?;
            let method = ["SetHandler(", "SetAction("]
                .into_iter()
                .find(|m| rest.starts_with(m))?;
            let (head, mut j) = read_ident(source, skip_trivia(b, after_var + 1 + method.len()));
            if head.is_empty() {
                return None;
            }
            let mut segs = vec![head];
            while b.get(j) == Some(&b'.') {
                let (seg, k) = read_ident(source, j + 1);
                if seg.is_empty() {
                    return None;
                }
                segs.push(seg);
                j = k;
            }
            if !matches!(b.get(skip_trivia(b, j)), Some(b',' | b')')) {
                return None;
            }
            match segs.as_slice() {
                [only] => Some(CallQualifier::Bare((*only).to_string())),
                [.., base, name] => Some(CallQualifier::Attribute {
                    base: (*base).to_string(),
                    name: (*name).to_string(),
                }),
                [] => None,
            }
        })
}

/// Spectre.Console.Cli: `.AddCommand<T>("x")` -> `cli:x` handled by `T`;
/// `.AddBranch("x", ...)` / `.AddBranch<TSettings>("x", ...)` -> `cli:x`.
fn scan_spectre_cli(source: &str, code: &CodeMap, out: &mut Vec<CliDecl>) {
    let b = source.as_bytes();
    for (needle, handled) in [(".AddCommand", true), (".AddBranch", false)] {
        let mut from = 0;
        while let Some(rel) = source.get(from..).and_then(|s| s.find(needle)) {
            let at = from + rel;
            from = at + needle.len();
            if !code.is_code(at) {
                continue;
            }
            let mut i = from;
            let mut ty = None;
            if b.get(i) == Some(&b'<') {
                let Some(close) = source[i..].find('>').filter(|&c| c <= 128).map(|c| i + c) else {
                    continue;
                };
                let inner = source[i + 1..close].trim();
                if inner.is_empty() || !inner.bytes().all(|c| is_ident_byte(c) || c == b'.') {
                    continue;
                }
                ty = inner.rsplit('.').next();
                i = close + 1;
            } else if handled {
                continue;
            }
            let open = skip_trivia(b, i);
            if b.get(open) != Some(&b'(') {
                continue;
            }
            let Some(name) = source
                .get(skip_trivia(b, open + 1)..)
                .and_then(extract_quoted)
            else {
                continue;
            };
            let handler = if handled {
                ty.map(|t| CallQualifier::Bare(t.to_string()))
            } else {
                None
            };
            out.push(CliDecl::new(
                CliFramework::SpectreCli,
                name,
                at_row(handler, source, at),
            ));
        }
    }
}

// ----------------------------------------------------------------------------
// LA.20b: scripting languages (python / ruby / php). Same contract as the
// LA.20a section: every slice is taken at an ASCII delimiter, so at a char
// boundary, and every scan is bounded.
// ----------------------------------------------------------------------------

/// Longest class body the Symfony / Laravel scan reads.
const MAX_CLASS_BODY: usize = 256 * 1024;
/// Longest string literal read whole (a Laravel `$signature`, a Thor usage).
const MAX_LITERAL: usize = 4096;

/// The literal and comment syntax [`CodeMap::script`] lexes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Script {
    Python,
    Ruby,
    Php,
    /// DH.10: `//` and nesting `/* */` comments, `'''` / `"""` blocks, raw
    /// `r'..'` / `r".."` strings (no escapes); `#` is code (a symbol literal).
    Dart,
    /// DH.10: `#` comments, `"""` / `'''` heredocs and `?c` char literals
    /// (`?"`, `?\n`); a `?` after an identifier byte ends a name (`valid?`).
    Elixir,
    /// FD.5b: JavaScript / TypeScript. `//` and (non-nesting) `/* */`
    /// comments, `'..'` / `".."` strings (a line each), and backtick template
    /// literals whose `${..}` spans are code, lexed recursively (a template
    /// nested in a span is a literal again). Regex literals are not lexed:
    /// a `/` is division or a comment opener, the one known gap (a regex
    /// holding a quote opens a string that ends at its line).
    Js,
    /// FD.5b: Go. `//` and `/* */` comments, `".."` strings, `'x'` runes and
    /// backtick raw strings (no escapes, no interpolation, any number of
    /// lines).
    Go,
    /// FD.5b: Kotlin. `//` and nesting `/* */` comments, `'x'` chars, and
    /// `".."` / `"""..."""` strings that all interpolate: `${..}` spans are
    /// code; a `$name` holds a name only, never a call, so it is text.
    Kotlin,
    /// FD.5b: Scala. [`Script::Kotlin`]'s comments and chars; `".."` and
    /// `"""..."""` strings interpolate only behind an interpolator
    /// (`s"..${..}"`, `f".."`, `raw".."`, any `id".."`), whose `${..}` spans
    /// are code (`$$` is a literal `$`).
    Scala,
    /// FD.5b: Swift. `//` and nesting `/* */` comments, `".."` and
    /// `"""..."""` strings with `\(..)` interpolation spans (code), and raw
    /// strings `#".."#` / `#"""..."""#` (any number of `#`), whose spans are
    /// `\#(..)`.
    Swift,
}

impl CodeMap {
    /// [`CodeMap::new`]'s contract for Python, Ruby, PHP, Dart and Elixir,
    /// whose literals and comments are not the C family's: `#` line comments
    /// (not PHP 8's `#[` attribute opener, and not in Dart), PHP's and Dart's
    /// `//` and `/* */` (Dart's nest), Ruby's `=begin` / `=end`, Python's,
    /// Dart's and Elixir's `'''` / `"""` blocks, `'...'` strings of any length
    /// (in the C family `'` opens a char literal), Dart raw strings, Elixir
    /// `?c` char literals, and heredoc bodies (Ruby `<<~ID` / `<<-ID`, PHP
    /// `<<<ID`) through their terminator line. Ruby `%w()` literals and
    /// Elixir sigils are read as code.
    ///
    /// FD.5b: [`Script::Js`], [`Script::Go`], [`Script::Kotlin`],
    /// [`Script::Scala`] and [`Script::Swift`] are lexed by [`lex_code`]: an
    /// interpolating literal is mapped as its text pieces only, so the
    /// bytes of a `${..}` / `\(..)` span are code in the map itself.
    pub(crate) fn script(source: &str, script: Script) -> Self {
        let b = source.as_bytes();
        if matches!(
            script,
            Script::Js | Script::Go | Script::Kotlin | Script::Scala | Script::Swift
        ) {
            let mut skips = Vec::new();
            lex_code(b, 0, script, &mut skips, None, 0);
            return CodeMap { skips };
        }
        let mut skips = Vec::new();
        let mut heredoc: Option<&[u8]> = None;
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\n'
                && let Some(id) = heredoc.take()
            {
                let end = heredoc_end(b, i + 1, id);
                skips.push((i + 1, end));
                i = end;
                continue;
            }
            let c_comments = matches!(script, Script::Php | Script::Dart);
            let triple = matches!(script, Script::Python | Script::Dart | Script::Elixir);
            let after_ident = i > 0 && is_ident_byte(b[i - 1]);
            let end = match b[i] {
                b'#' if script == Script::Php && b.get(i + 1) == Some(&b'[') => None,
                b'#' if script == Script::Dart => None,
                b'#' => Some(line_end(b, i)),
                b'/' if c_comments && b.get(i + 1) == Some(&b'/') => Some(line_end(b, i)),
                b'/' if script == Script::Php && b.get(i + 1) == Some(&b'*') => {
                    Some(skip_comment(b, i))
                }
                b'/' if script == Script::Dart && b.get(i + 1) == Some(&b'*') => {
                    Some(skip_nested_block_comment(b, i))
                }
                b'r' if script == Script::Dart
                    && !after_ident
                    && matches!(b.get(i + 1), Some(b'"' | b'\'')) =>
                {
                    let q = b[i + 1];
                    Some(if b[i + 1..].starts_with(&[q, q, q]) {
                        find_bytes(b, i + 4, &[q, q, q]).map_or(b.len(), |j| j + 3)
                    } else {
                        find_bytes(b, i + 2, &[q]).map_or(b.len(), |j| j + 1)
                    })
                }
                b'?' if script == Script::Elixir && !after_ident && i + 1 < b.len() => {
                    Some(elixir_char_end(b, i))
                }
                q @ (b'"' | b'\'') if triple && b[i..].starts_with(&[q, q, q]) => {
                    Some(find_bytes(b, i + 3, &[q, q, q]).map_or(b.len(), |j| j + 3))
                }
                q @ (b'"' | b'\'') => Some(skip_quoted(b, i, q)),
                b'=' if script == Script::Ruby
                    && (i == 0 || b[i - 1] == b'\n')
                    && b[i..].starts_with(b"=begin") =>
                {
                    Some(find_bytes(b, i, b"\n=end").map_or(b.len(), |j| line_end(b, j + 1)))
                }
                b'<' => {
                    if let Some((id, after)) = heredoc_opener(b, i, script) {
                        heredoc = Some(id);
                        i = after;
                        continue;
                    }
                    None
                }
                _ => None,
            };
            match end {
                Some(end) if end > i => {
                    skips.push((i, end));
                    i = end;
                }
                _ => i += 1,
            }
        }
        CodeMap { skips }
    }

    /// The `[start, end)` literal / comment ranges, for tests of the lexers.
    #[cfg(test)]
    pub(crate) fn skips(&self) -> &[(usize, usize)] {
        &self.skips
    }

    /// `pos` when it is code, else the end of the literal or comment holding it.
    fn next_code(&self, pos: usize) -> usize {
        let i = self.skips.partition_point(|&(start, _)| start <= pos);
        match i.checked_sub(1).map(|k| self.skips[k]) {
            Some((_, end)) if end > pos => end,
            _ => pos,
        }
    }
}

/// FD.5b: deepest nesting of interpolation spans [`lex_code`] follows; a
/// `${` / `\(` deeper than this is read as literal text.
const MAX_INTERP_DEPTH: usize = 32;

/// FD.5b: how an interpolating literal marks a code span.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Interp {
    /// No span: the literal is text whole.
    None,
    /// `${..}` (JS templates, Kotlin, Scala interpolators); `scala` reads
    /// `$$` as an escaped `$`.
    Dollar { scala: bool },
    /// Swift `\(..)`, or `\#(..)` in a raw string of `hashes` `#`s.
    Paren { hashes: usize },
}

/// FD.5b: one literal's syntax for [`lex_literal`].
#[derive(Clone, Copy)]
struct LitSyntax<'c> {
    /// The closing delimiter.
    close: &'c [u8],
    /// `\` escapes the next byte (Swift raw strings: `\` then `hashes` `#`).
    escapes: bool,
    hashes: usize,
    interp: Interp,
    /// The literal ends at its line (an unterminated `'..'` / `".."`).
    single_line: bool,
    /// Kotlin / Scala `"""..""""`: a run of extra quotes before the closer
    /// belongs to the literal.
    greedy_close: bool,
}

/// FD.5b: lex code from `i` for [`CodeMap::script`]'s compiled / JS arms,
/// pushing each literal and comment to `skips` in source order. With a
/// `closer` (`}` of a `${..}` span, `)` of a Swift `\(..)` span) it stops at
/// the first unmatched closer and returns its index; else it runs to the end
/// of input.
fn lex_code(
    b: &[u8],
    mut i: usize,
    script: Script,
    skips: &mut Vec<(usize, usize)>,
    closer: Option<u8>,
    depth: usize,
) -> usize {
    let opener = closer.map(|c| if c == b')' { b'(' } else { b'{' });
    let nesting = matches!(script, Script::Kotlin | Script::Scala | Script::Swift);
    let mut nest = 0usize;
    while i < b.len() {
        let c = b[i];
        if Some(c) == closer {
            if nest == 0 {
                return i;
            }
            nest -= 1;
            i += 1;
            continue;
        }
        if Some(c) == opener {
            nest += 1;
            i += 1;
            continue;
        }
        let after_ident = i > 0 && is_ident_byte(b[i - 1]);
        let plain = |close: &'static [u8], single_line: bool| LitSyntax {
            close,
            escapes: true,
            hashes: 0,
            interp: Interp::None,
            single_line,
            greedy_close: false,
        };
        let end = match c {
            b'/' if b.get(i + 1) == Some(&b'/') => Some(line_end(b, i)),
            b'/' if b.get(i + 1) == Some(&b'*') => Some(if nesting {
                skip_nested_block_comment(b, i)
            } else {
                skip_comment(b, i)
            }),
            b'`' if script == Script::Js => {
                let syntax = LitSyntax {
                    interp: Interp::Dollar { scala: false },
                    ..plain(b"`", false)
                };
                Some(lex_literal(b, i, i + 1, syntax, script, skips, depth))
            }
            b'`' if script == Script::Go => {
                Some(find_bytes(b, i + 1, b"`").map_or(b.len(), |j| j + 1))
            }
            q @ (b'\'' | b'"') if script == Script::Js => {
                let close: &'static [u8] = if q == b'\'' { b"'" } else { b"\"" };
                Some(lex_literal(
                    b,
                    i,
                    i + 1,
                    plain(close, true),
                    script,
                    skips,
                    depth,
                ))
            }
            b'\'' if script != Script::Swift => char_literal_end(b, i),
            b'"' if script == Script::Go => Some(lex_literal(
                b,
                i,
                i + 1,
                plain(b"\"", true),
                script,
                skips,
                depth,
            )),
            b'"' if matches!(script, Script::Kotlin | Script::Scala) => {
                let interp = if script == Script::Kotlin || after_ident {
                    Interp::Dollar {
                        scala: script == Script::Scala,
                    }
                } else {
                    Interp::None
                };
                let syntax = if b[i..].starts_with(b"\"\"\"") {
                    LitSyntax {
                        escapes: false,
                        interp,
                        greedy_close: true,
                        ..plain(b"\"\"\"", false)
                    }
                } else {
                    LitSyntax {
                        interp,
                        ..plain(b"\"", true)
                    }
                };
                let body = i + syntax.close.len();
                Some(lex_literal(b, i, body, syntax, script, skips, depth))
            }
            b'"' | b'#' if script == Script::Swift => swift_literal(b, i, skips, depth),
            _ => None,
        };
        match end {
            Some(end) if end > i => {
                if skips.last().is_none_or(|&(s, _)| s < i) {
                    skips.push((i, end));
                }
                i = end;
            }
            _ => i += 1,
        }
    }
    b.len()
}

/// FD.5b: a Swift string literal at `at` (`".."`, `"""..."""`, or a raw
/// `#".."#` / `#"""..."""#`), lexed by [`lex_literal`]; `None` when a `#`
/// opens no raw string (`#if`, `#selector(`).
fn swift_literal(
    b: &[u8],
    at: usize,
    skips: &mut Vec<(usize, usize)>,
    depth: usize,
) -> Option<usize> {
    let mut q = at;
    while b.get(q) == Some(&b'#') {
        q += 1;
    }
    if b.get(q) != Some(&b'"') {
        return None;
    }
    let hashes = q - at;
    let triple = b[q..].starts_with(b"\"\"\"");
    let mut close = if triple {
        b"\"\"\"".to_vec()
    } else {
        b"\"".to_vec()
    };
    close.extend(std::iter::repeat_n(b'#', hashes));
    let syntax = LitSyntax {
        close: &close,
        escapes: true,
        hashes,
        interp: Interp::Paren { hashes },
        single_line: !triple,
        greedy_close: false,
    };
    let body = q + if triple { 3 } else { 1 };
    Some(lex_literal(
        b,
        at,
        body,
        syntax,
        Script::Swift,
        skips,
        depth,
    ))
}

/// FD.5b: lex the literal opening at `open` (its first content byte at
/// `body`) and return the index just past it. Its text is pushed to `skips`
/// as one piece per run between interpolation spans; each span is lexed as
/// code by [`lex_code`] (literals nested in it are pushed in order), so a
/// span's bytes are code in the map. An unterminated literal ends at the end
/// of input, or at its line when `single_line`.
fn lex_literal(
    b: &[u8],
    open: usize,
    body: usize,
    syntax: LitSyntax<'_>,
    script: Script,
    skips: &mut Vec<(usize, usize)>,
    depth: usize,
) -> usize {
    let push = |skips: &mut Vec<(usize, usize)>, s: usize, e: usize| {
        if e > s {
            skips.push((s, e));
        }
    };
    let mut seg = open;
    let mut j = body.min(b.len());
    let spans = depth < MAX_INTERP_DEPTH;
    while j < b.len() {
        if syntax.single_line && b[j] == b'\n' {
            push(skips, seg, j);
            return j;
        }
        if b[j..].starts_with(syntax.close) {
            let mut end = j + syntax.close.len();
            if syntax.greedy_close {
                while b.get(end) == Some(&b'"') {
                    end += 1;
                }
            }
            push(skips, seg, end);
            return end;
        }
        if syntax.escapes
            && b[j] == b'\\'
            && b.get(j + 1..j + 1 + syntax.hashes)
                .is_some_and(|h| h.iter().all(|&c| c == b'#'))
        {
            let after = j + 1 + syntax.hashes;
            if spans && matches!(syntax.interp, Interp::Paren { .. }) && b.get(after) == Some(&b'(')
            {
                push(skips, seg, after + 1);
                let k = lex_code(b, after + 1, script, skips, Some(b')'), depth + 1);
                if k >= b.len() {
                    return b.len();
                }
                seg = k;
                j = k + 1;
                continue;
            }
            j = after + 1;
            continue;
        }
        if let Interp::Dollar { scala } = syntax.interp
            && b[j] == b'$'
        {
            if scala && b.get(j + 1) == Some(&b'$') {
                j += 2;
                continue;
            }
            if spans && b.get(j + 1) == Some(&b'{') {
                push(skips, seg, j + 2);
                let k = lex_code(b, j + 2, script, skips, Some(b'}'), depth + 1);
                if k >= b.len() {
                    return b.len();
                }
                seg = k;
                j = k + 1;
                continue;
            }
        }
        j += 1;
    }
    push(skips, seg, b.len());
    b.len()
}

/// Index just past the Elixir char literal whose `?` is at `at` (`?a`,
/// `?\\n`, `?é`), on a char boundary.
fn elixir_char_end(b: &[u8], at: usize) -> usize {
    let width = |lead: u8| match lead {
        c if c < 0x80 => 1,
        c if c >= 0xF0 => 4,
        c if c >= 0xE0 => 3,
        _ => 2,
    };
    let end = match b[at + 1] {
        b'\\' => at + 2 + b.get(at + 2).map_or(0, |&c| width(c)),
        c => at + 1 + width(c),
    };
    end.min(b.len())
}

/// The index of the `\n` ending the line `at` is on, or the end of input.
fn line_end(b: &[u8], at: usize) -> usize {
    find_bytes(b, at, b"\n").unwrap_or(b.len())
}

/// Index just past the `q`-quoted string at `at` (`\` escapes honoured); the
/// end of input when it never closes.
fn skip_quoted(b: &[u8], at: usize, q: u8) -> usize {
    let mut j = at + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            c if c == q => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

/// A heredoc opener at `at` — Ruby `<<~ID` / `<<-ID` (the identifier may be
/// quoted), PHP `<<<ID` / `<<<"ID"` / `<<<'ID'` — as its identifier and the
/// index after it. A bare Ruby `<<` is the append operator, not an opener.
fn heredoc_opener(b: &[u8], at: usize, script: Script) -> Option<(&[u8], usize)> {
    let mut i = match script {
        Script::Ruby if b[at..].starts_with(b"<<~") || b[at..].starts_with(b"<<-") => at + 3,
        Script::Php if b[at..].starts_with(b"<<<") => skip_ws(b, at + 3),
        _ => return None,
    };
    let quote = b
        .get(i)
        .copied()
        .filter(|c| matches!(c, b'"' | b'\'' | b'`'));
    if quote.is_some() {
        i += 1;
    }
    let start = i;
    if !b
        .get(i)
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
    {
        return None;
    }
    while b.get(i).copied().is_some_and(is_ident_byte) {
        i += 1;
    }
    let id = &b[start..i];
    match quote {
        Some(q) if b.get(i) == Some(&q) => Some((id, i + 1)),
        Some(_) => None,
        None => Some((id, i)),
    }
}

/// End of the heredoc body starting at `from`: past the first line whose
/// trimmed text starts with `id` and continues with no identifier byte (PHP
/// allows `ID;` / `ID,` / `ID)`), or the end of input.
fn heredoc_end(b: &[u8], from: usize, id: &[u8]) -> usize {
    let mut line = from;
    while line < b.len() {
        let end = line_end(b, line);
        let text = skip_ws(b, line);
        if text < end
            && b[text..end].starts_with(id)
            && !b.get(text + id.len()).copied().is_some_and(is_ident_byte)
        {
            return end;
        }
        line = end + 1;
    }
    b.len()
}

/// The first index at or after `i` that is not ASCII whitespace.
fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while b.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

/// The first index at or after `i` that is neither whitespace nor inside a
/// literal or comment of `code`. Only where no string is expected: it steps
/// over string literals too.
fn skip_trivia_code(b: &[u8], code: &CodeMap, mut i: usize) -> usize {
    loop {
        let j = code.next_code(skip_ws(b, i));
        if j == i {
            return i;
        }
        i = j;
    }
}

fn closer_of(open: u8) -> u8 {
    match open {
        b'(' => b')',
        b'[' => b']',
        _ => b'}',
    }
}

/// [`balanced`] for a [`CodeMap::script`] file: brackets count only at code
/// bytes, so a `)` in a `'...'` string or a `#` comment does not close it.
fn balanced_code(
    source: &str,
    code: &CodeMap,
    open_at: usize,
    cap: usize,
) -> Option<(usize, usize)> {
    let b = source.as_bytes();
    let first = *b.get(open_at)?;
    if !matches!(first, b'(' | b'[' | b'{') || !code.is_code(open_at) {
        return None;
    }
    let limit = b.len().min(open_at.saturating_add(cap));
    let mut stack = vec![closer_of(first)];
    let mut i = open_at + 1;
    while i < limit {
        let next = code.next_code(i);
        if next != i {
            i = next;
            continue;
        }
        match b[i] {
            c @ (b'(' | b'[' | b'{') => stack.push(closer_of(c)),
            c @ (b')' | b']' | b'}') => {
                if stack.pop()? != c {
                    return None;
                }
                if stack.is_empty() {
                    return Some((open_at + 1, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The argument span `(start, end)` of the call whose name ends at `after`
/// (`add_parser` + `("x")`), whitespace allowed before the `(`.
fn call_args(source: &str, code: &CodeMap, after: usize) -> Option<(usize, usize)> {
    let open = skip_ws(source.as_bytes(), after);
    balanced_code(source, code, open, MAX_ATTR_SPAN).filter(|_| source.as_bytes()[open] == b'(')
}

/// Byte offsets in `source[start..end]` where `word` stands whole at a code
/// byte: no identifier byte or `$` before it, no identifier byte after it.
/// `word` may begin with `$` (a PHP variable).
fn code_words(source: &str, code: &CodeMap, start: usize, end: usize, word: &str) -> Vec<usize> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut from = start;
    while let Some(rel) = source.get(from..end).and_then(|s| s.find(word)) {
        let at = from + rel;
        from = at + word.len();
        if code.is_code(at)
            && (at == 0 || !(is_ident_byte(b[at - 1]) || b[at - 1] == b'$'))
            && !b.get(at + word.len()).copied().is_some_and(is_ident_byte)
        {
            out.push(at);
        }
    }
    out
}

/// The quoted value of `key <sep> "v"` (Python `name="x"`, PHP `name: 'x'`)
/// at a code position of the span `(start, end)`. Not `==` / `::`.
fn kwarg_str(
    source: &str,
    code: &CodeMap,
    (start, end): (usize, usize),
    key: &str,
    sep: u8,
) -> Option<String> {
    let b = source.as_bytes();
    code_words(source, code, start, end, key)
        .into_iter()
        .find_map(|at| {
            let i = skip_ws(b, at + key.len());
            if b.get(i) != Some(&sep) || matches!(b.get(i + 1), Some(b'=' | b':')) {
                return None;
            }
            source.get(skip_ws(b, i + 1)..end).and_then(extract_quoted)
        })
}

/// The first argument of the span `(start, end)` when it is exactly a string
/// literal (`("x")`, `("x", help=...)`), not `("x" + y)`.
fn first_positional_str(source: &str, (start, end): (usize, usize)) -> Option<String> {
    let b = source.as_bytes();
    let v = skip_ws(b, start);
    let name = source.get(v..end).and_then(extract_quoted)?;
    let after = skip_ws(b, v + name.len() + 2);
    (after >= end || b[after] == b',').then_some(name)
}

/// The identifier ending just before byte `end` (empty when there is none).
fn ident_before(source: &str, end: usize) -> &str {
    let b = source.as_bytes();
    let mut i = end;
    while i > 0 && is_ident_byte(b[i - 1]) {
        i -= 1;
    }
    source.get(i..end).unwrap_or("")
}

/// `Bare(f)` for `f`, `Attribute { m, f }` for `pkg.m.f`.
fn callable_qualifier(segs: &[&str]) -> Option<CallQualifier> {
    match segs {
        [only] => Some(CallQualifier::Bare((*only).to_string())),
        [.., base, name] => Some(CallQualifier::Attribute {
            base: (*base).to_string(),
            name: (*name).to_string(),
        }),
        [] => None,
    }
}

/// A command name read out of a longer literal: non-empty, at most 64 bytes,
/// no whitespace, quote, `$`, brace or backslash (an interpolation or an
/// argument spec is not part of a name).
fn valid_cli_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.contains(|c: char| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '$' | '{' | '}' | '\\')
        })
}

/// The raw text inside the `'...'` / `"..."` literal `s` starts with (`\`
/// escapes stepped over, not decoded), at most [`MAX_LITERAL`] bytes.
fn quoted_literal(s: &str) -> Option<&str> {
    let b = s.as_bytes();
    let q = *b.first()?;
    if q != b'"' && q != b'\'' {
        return None;
    }
    let limit = b.len().min(MAX_LITERAL + 2);
    let mut j = 1;
    while j < limit {
        match b[j] {
            b'\\' => j += 2,
            c if c == q => return s.get(1..j),
            _ => j += 1,
        }
    }
    None
}

// ---- python: Typer / click / argparse ---------------------------------------

/// True when an `import m`, `import m.x as y`, `from m import ...` or
/// `from m.x import ...` line names the top-level module `module`.
fn py_imports(source: &str, module: &str) -> bool {
    let names = |item: &str| {
        item.trim()
            .split(|c: char| c == '.' || c.is_whitespace())
            .next()
            == Some(module)
    };
    source.lines().any(|line| {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("import ") {
            rest.split(',').any(names)
        } else {
            t.strip_prefix("from ").is_some_and(names)
        }
    })
}

/// Which library a `@<obj>.command` belongs to: `click` itself, else the one
/// library the file imports; with both, Typer when `obj` is a `typer.Typer()`;
/// with neither, LA.20a's rule (Typer when the file mentions typer).
fn py_cli_framework(source: &str, obj: &str, typer: bool, click: bool) -> CliFramework {
    let typer_app = || {
        source.contains(&format!("{obj} = typer.Typer("))
            || source.contains(&format!("{obj} = Typer("))
    };
    match (typer, click) {
        _ if obj == "click" => CliFramework::Click,
        (true, false) => CliFramework::Typer,
        (false, true) => CliFramework::Click,
        (true, true) if typer_app() => CliFramework::Typer,
        (false, false) if source.contains("typer") => CliFramework::Typer,
        _ => CliFramework::Click,
    }
}

/// The name Typer and click give a command from its function: lowercased,
/// `_` -> `-` (Typer's `get_command_name`; click >= 7). click 8.2 also drops a
/// trailing `-command` / `-cmd` / `-group` / `-grp` word.
fn py_default_command_name(func: &str, framework: CliFramework) -> String {
    let name = func.to_lowercase().replace('_', "-");
    if framework == CliFramework::Click
        && let Some((head, suffix)) = name.rsplit_once('-')
        && matches!(suffix, "command" | "cmd" | "group" | "grp")
        && !head.is_empty()
    {
        return head.to_string();
    }
    name
}

/// The top-level `def` / `async def` / `class` names of a Python file: the
/// symbols its MODULE indexes by name.
///
/// LA.20b stopgap: a CLI_COMMAND named like one of them (`@app.command() def
/// main` -> `cli:main`, `add_parser("migrate")` beside `def migrate`) would
/// replace that symbol in `module_symbols` (graph/src/build.rs indexes every
/// module child, the command last), so `from tool.cli import main` and every
/// call through it would bind to the command. The forms this packet adds
/// (bare decorators, `add_typer`, argparse) skip such a name. Remove once
/// `build_symbol_table` stops indexing CLI_COMMAND children into
/// `module_symbols`.
fn py_module_defs<'a>(source: &'a str, code: &CodeMap) -> std::collections::HashSet<&'a str> {
    let mut out = std::collections::HashSet::new();
    let mut off = 0;
    for line in source.split_inclusive('\n') {
        if code.is_code(off) {
            let rest = line.strip_prefix("async ").unwrap_or(line);
            let word = ["def ", "class "]
                .into_iter()
                .find_map(|kw| rest.strip_prefix(kw));
            if let Some(after) = word {
                let (name, _) = read_ident(after, skip_ws(after.as_bytes(), 0));
                if !name.is_empty() {
                    out.insert(name);
                }
            }
        }
        off += line.len();
    }
    out
}

/// An explicit Typer / click command name in a `.command(...)` span, and
/// whether this packet added it: the first positional string (minted before
/// LA.20b, by commander's needle) or `name="x"` (new).
fn py_explicit_name(source: &str, code: &CodeMap, span: (usize, usize)) -> Option<(String, bool)> {
    first_positional_str(source, span)
        .map(|name| (name, false))
        .or_else(|| kwarg_str(source, code, span, "name", b'=').map(|name| (name, true)))
}

/// The command name for a `.command(...)` on function `func`: the explicit
/// name, else (when `defaults` allows it) the library's name for `func`. A
/// name this packet adds is skipped when it equals a top-level def
/// ([`py_module_defs`]).
fn py_command_name(
    explicit: Option<(String, bool)>,
    func: Option<&str>,
    framework: CliFramework,
    defaults: bool,
    defs: &std::collections::HashSet<&str>,
) -> Option<String> {
    let (name, new) = match (explicit, func) {
        (Some(explicit), _) => explicit,
        (None, Some(f)) if defaults => (py_default_command_name(f, framework), true),
        _ => return None,
    };
    (!new || !defs.contains(name.as_str())).then_some(name)
}

/// The dotted identifier that exactly fills the span `(start, end)` (`fn`,
/// `cmds.fn`), as its segments.
fn dotted_exact(source: &str, (start, end): (usize, usize)) -> Option<Vec<&str>> {
    let b = source.as_bytes();
    let mut j = skip_ws(b, start);
    let mut segs = Vec::new();
    loop {
        let (seg, k) = read_ident(source, j);
        if seg.is_empty() {
            return None;
        }
        segs.push(seg);
        j = k;
        if b.get(j) != Some(&b'.') {
            break;
        }
        j += 1;
    }
    (skip_ws(b, j) == end).then_some(segs)
}

/// True when only spaces / tabs precede byte `at` on its line.
fn line_head(b: &[u8], at: usize) -> bool {
    b[..at]
        .iter()
        .rev()
        .take_while(|&&c| c != b'\n')
        .all(|&c| c == b' ' || c == b'\t')
}

/// The function a Python decorator ending at `from` decorates: past further
/// decorators (argument lists may span lines) and comments to `def name` /
/// `async def name`.
fn py_decorated_function<'a>(source: &'a str, code: &CodeMap, from: usize) -> Option<&'a str> {
    let b = source.as_bytes();
    let mut i = from;
    for _ in 0..32 {
        i = skip_trivia_code(b, code, i);
        if b.get(i) == Some(&b'@') {
            let (end, _) = read_dotted(source, i + 1);
            let open = skip_ws(b, end);
            i = match balanced_code(source, code, open, MAX_ATTR_SPAN) {
                Some((_, close)) if b[open] == b'(' => close + 1,
                _ => end,
            };
            continue;
        }
        let (word, after) = read_ident(source, i);
        match word {
            "async" => i = after,
            "def" => {
                let (name, _) = read_ident(source, skip_ws(b, after));
                return (!name.is_empty()).then_some(name);
            }
            _ => return None,
        }
    }
    None
}

/// Typer and click: every `@<obj>.command(...)` decorator (`@click.command`,
/// a group's `@cli.command`, Typer's `@app.command`) -> `cli:<name>`, handled
/// by the decorated function. The name is the first positional string or
/// `name="x"`. A bare decorator (`@app.command()`, and click 8.1's paren-less
/// `@click.command`) takes the function's name the way the library does, and
/// is read only in a file that imports typer or click. In a Typer file,
/// `app.add_typer(sub, name="x")` -> `cli:x`. A group (`@click.group()`,
/// `@cli.group()`) is the binary or a namespace and is never minted here. A
/// bare or `add_typer` name equal to a top-level def is skipped
/// ([`py_module_defs`]).
fn scan_typer_click(source: &str, code: &CodeMap) -> Vec<CliDecl> {
    let (typer, click) = (py_imports(source, "typer"), py_imports(source, "click"));
    let defs = py_module_defs(source, code);
    let b = source.as_bytes();
    let mut found: Vec<(usize, CliDecl)> = Vec::new();
    let mut from = 0;
    while let Some(rel) = source.get(from..).and_then(|s| s.find('@')) {
        let at = from + rel;
        from = at + 1;
        if !code.is_code(at) || !line_head(b, at) {
            continue;
        }
        let (name_end, last) = read_dotted(source, at + 1);
        let Some(obj) = source
            .get(at + 1..name_end)
            .and_then(|d| d.strip_suffix(".command"))
        else {
            continue;
        };
        if last != "command" {
            continue;
        }
        let framework = py_cli_framework(source, obj, typer, click);
        let (explicit, end) = if b.get(skip_ws(b, name_end)) == Some(&b'(') {
            let Some(span) = call_args(source, code, name_end) else {
                continue;
            };
            (py_explicit_name(source, code, span), span.1 + 1)
        } else if click {
            (None, name_end)
        } else {
            continue;
        };
        from = end;
        let func = py_decorated_function(source, code, end);
        let Some(name) = py_command_name(explicit, func, framework, typer || click, &defs) else {
            continue;
        };
        let handler = func.map(|f| CallQualifier::Bare(f.to_string()));
        found.push((
            at,
            CliDecl::new(framework, name, at_row(handler, source, at)),
        ));
    }
    if typer || click {
        // `app.command("x")(fn)`: the decorator applied by hand.
        for at in code_words(source, code, 0, source.len(), "command") {
            if at == 0 || b[at - 1] != b'.' {
                continue;
            }
            let mut recv = at - 1;
            while recv > 0 && (is_ident_byte(b[recv - 1]) || b[recv - 1] == b'.') {
                recv -= 1;
            }
            let obj = &source[recv..at - 1];
            if obj.is_empty() || (recv > 0 && b[recv - 1] == b'@') {
                continue;
            }
            let Some(span) = call_args(source, code, at + "command".len()) else {
                continue;
            };
            let Some(target) = call_args(source, code, span.1 + 1) else {
                continue;
            };
            let Some(segs) = dotted_exact(source, target) else {
                continue;
            };
            let framework = py_cli_framework(source, obj, typer, click);
            let func = segs.last().copied();
            let explicit = py_explicit_name(source, code, span);
            let Some(name) = py_command_name(explicit, func, framework, true, &defs) else {
                continue;
            };
            found.push((
                at,
                CliDecl::new(
                    framework,
                    name,
                    at_row(callable_qualifier(&segs), source, at),
                ),
            ));
        }
    }
    if typer {
        for at in code_words(source, code, 0, source.len(), "add_typer") {
            if at > 0
                && b[at - 1] == b'.'
                && let Some(span) = call_args(source, code, at + "add_typer".len())
                && let Some(name) = kwarg_str(source, code, span, "name", b'=')
                && !defs.contains(name.as_str())
            {
                found.push((at, CliDecl::new(CliFramework::Typer, name, None)));
            }
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, decl)| decl).collect()
}

/// The callable passed as `func=` (or `handler=`) in a `set_defaults(...)`
/// span: `Bare(H)` / `Attribute { m, H }` for `m.H`. A lambda or a call is
/// not a handler.
fn py_set_defaults_handler(
    source: &str,
    code: &CodeMap,
    (start, end): (usize, usize),
) -> Option<CallQualifier> {
    let b = source.as_bytes();
    ["func", "handler"].into_iter().find_map(|key| {
        code_words(source, code, start, end, key)
            .into_iter()
            .find_map(|at| {
                let i = skip_ws(b, at + key.len());
                if b.get(i) != Some(&b'=') || b.get(i + 1) == Some(&b'=') {
                    return None;
                }
                let mut j = skip_ws(b, i + 1);
                let mut segs = Vec::new();
                loop {
                    let (seg, k) = read_ident(source, j);
                    if seg.is_empty() || seg == "lambda" {
                        return None;
                    }
                    segs.push(seg);
                    j = k;
                    if b.get(j) != Some(&b'.') {
                        break;
                    }
                    j += 1;
                }
                let k = skip_ws(b, j);
                if k < end && b[k] != b',' {
                    return None;
                }
                callable_qualifier(&segs)
            })
    })
}

/// argparse, in a file that imports it: `ArgumentParser(prog="x")` -> `cli:x`
/// (the binary); each `<subparsers>.add_parser("x")` -> `cli:x`, handled by the
/// `func=` of a `set_defaults` on it, chained or on the variable the parser
/// was assigned to (`m = sub.add_parser("x")` ... `m.set_defaults(func=H)`),
/// read against that variable's nearest preceding assignment. A name equal to
/// a top-level def is skipped ([`py_module_defs`]).
fn scan_argparse(source: &str, code: &CodeMap) -> Vec<CliDecl> {
    if !py_imports(source, "argparse") {
        return Vec::new();
    }
    let defs = py_module_defs(source, code);
    let b = source.as_bytes();
    let len = source.len();
    let mut out = Vec::new();
    // (position, variable, index into `out`) of each assigned subparser; the
    // index is `None` for one that minted nothing, so a later `set_defaults`
    // on the reused variable never lands on an earlier command.
    let mut vars: Vec<(usize, &str, Option<usize>)> = Vec::new();
    let mut calls: Vec<(usize, bool)> = code_words(source, code, 0, len, "ArgumentParser")
        .into_iter()
        .map(|at| (at, true))
        .chain(
            code_words(source, code, 0, len, "add_parser")
                .into_iter()
                .map(|at| (at, false)),
        )
        .collect();
    calls.sort_unstable();
    for (at, root) in calls {
        if root {
            if let Some(span) = call_args(source, code, at + "ArgumentParser".len())
                && let Some(name) = kwarg_str(source, code, span, "prog", b'=')
                && !defs.contains(name.as_str())
            {
                out.push(CliDecl::new(CliFramework::Argparse, name, None));
            }
            continue;
        }
        if at == 0 || b[at - 1] != b'.' {
            continue;
        }
        let mut recv = at - 1;
        while recv > 0 && (is_ident_byte(b[recv - 1]) || b[recv - 1] == b'.') {
            recv -= 1;
        }
        let var = assigned_variable(source, recv);
        let name = call_args(source, code, at + "add_parser".len()).and_then(|span| {
            let name = first_positional_str(source, span)
                .or_else(|| kwarg_str(source, code, span, "name", b'='))?;
            (!defs.contains(name.as_str())).then_some((span, name))
        });
        let minted = name.map(|(span, name)| {
            let chained = source
                .get(span.1 + 1..)
                .filter(|rest| rest.starts_with(".set_defaults"))
                .and_then(|_| call_args(source, code, span.1 + 1 + ".set_defaults".len()))
                .and_then(|defaults| py_set_defaults_handler(source, code, defaults));
            out.push(CliDecl::new(
                CliFramework::Argparse,
                name,
                at_row(chained, source, at),
            ));
            out.len() - 1
        });
        if let Some(var) = var {
            vars.push((at, var, minted));
        }
    }
    for at in code_words(source, code, 0, len, "set_defaults") {
        if at == 0 || b[at - 1] != b'.' {
            continue;
        }
        let var = ident_before(source, at - 1);
        let Some(&(_, _, Some(idx))) = vars.iter().rev().find(|(pos, v, _)| *pos < at && *v == var)
        else {
            continue;
        };
        if let Some(decl) = out.get_mut(idx)
            && decl.handler.is_none()
            && let Some(span) = call_args(source, code, at + "set_defaults".len())
        {
            decl.handler = at_row(py_set_defaults_handler(source, code, span), source, at);
        }
    }
    out
}

// ---- ruby: Thor ---------------------------------------------------------------

/// `class Deployer < Thor` (or `< ::Thor`) -> `Deployer`; `class A::Cli <
/// Thor` -> `Cli`. `Thor::Group` runs its methods in sequence and declares no
/// subcommands, so it is not a Thor class here.
fn thor_class(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("class")?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let (name, parent) = rest.split_once('<')?;
    let parent = parent.trim_start();
    let after = parent
        .strip_prefix("::")
        .unwrap_or(parent)
        .strip_prefix("Thor")?;
    if after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == ':') {
        return None;
    }
    let last = name.trim().rsplit("::").next()?;
    (last.starts_with(|c: char| c.is_ascii_uppercase()) && last.bytes().all(is_ident_byte))
        .then_some(last)
}

/// The string a Ruby call's first argument is (`desc "x", ...`,
/// `desc("x", ...)`), and the text after it.
fn ruby_first_string(rest: &str) -> Option<(&str, &str)> {
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('(').map_or(rest, str::trim_start);
    let lit = quoted_literal(rest)?;
    Some((lit, rest.get(lit.len() + 2..)?))
}

/// Thor, in a `class X < Thor`: each `desc "<usage>", "<help>"` describes the
/// next public instance method, which becomes `cli:<name>` handled by
/// `Attribute { X, method }`. The name is the usage's first word when it
/// spells the method (Thor dispatches `rollout-all` to `rollout_all`), else
/// the method name. A `def` with no `desc` is not a command (Thor warns and
/// skips it), and a `private` / `protected` method, `def self.x`,
/// `initialize` or a method inside `no_commands` does not take a pending
/// `desc` (Thor's `method_added` ignores them). `subcommand "x", Klass` ->
/// `cli:x` handled by `Klass`. The body is read by indentation: members sit at
/// the first body line's indent, and the class ends at a line indented no
/// deeper than `class`. `map "-v" => :version` aliases are not read.
fn scan_thor(source: &str, code: &CodeMap) -> Vec<CliDecl> {
    let mut lines: Vec<(usize, &str)> = Vec::new();
    let mut off = 0;
    for line in source.split_inclusive('\n') {
        lines.push((off, line.trim_end()));
        off += line.len();
    }
    let indent_of = |l: &str| l.len() - l.trim_start().len();
    let mut out = Vec::new();
    for (k, &(off, line)) in lines.iter().enumerate() {
        let indent = indent_of(line);
        if !code.is_code(off + indent) {
            continue;
        }
        let Some(class) = thor_class(&line[indent..]) else {
            continue;
        };
        let mut member: Option<usize> = None;
        let mut pending: Option<&str> = None;
        let mut private = false;
        for &(off, line) in &lines[k + 1..] {
            let ind = indent_of(line);
            if ind == line.len() || !code.is_code(off + ind) {
                continue;
            }
            if ind <= indent {
                break;
            }
            if ind != *member.get_or_insert(ind) {
                continue;
            }
            let t = &line[ind..];
            let (word, after) = read_ident(t, 0);
            let rest = t[after..].trim_start();
            match word {
                "private" | "protected" if rest.is_empty() => private = true,
                "public" if rest.is_empty() => private = false,
                "no_commands" => pending = None,
                "desc" if !rest.starts_with([':', '=']) => {
                    pending = ruby_first_string(rest)
                        .and_then(|(usage, _)| usage.split_whitespace().next());
                }
                "subcommand" => {
                    let Some((name, tail)) = ruby_first_string(rest) else {
                        continue;
                    };
                    if !valid_cli_name(name) {
                        continue;
                    }
                    let klass = tail
                        .trim_start()
                        .strip_prefix(',')
                        .map(str::trim_start)
                        .and_then(|k| {
                            let end = k.find(|c: char| {
                                !(c.is_ascii_alphanumeric() || c == '_' || c == ':')
                            });
                            k[..end.unwrap_or(k.len())]
                                .rsplit("::")
                                .next()
                                .filter(|s| !s.is_empty())
                        });
                    let handler = klass.map(|k| CallQualifier::Bare(k.to_string()));
                    out.push(CliDecl::new(
                        CliFramework::Thor,
                        name.to_string(),
                        at_row(handler, source, off),
                    ));
                }
                "def" => {
                    let (method, end) = read_ident(t, skip_ws(t.as_bytes(), after));
                    let singleton_or_op =
                        matches!(t.as_bytes().get(end), Some(b'.' | b'?' | b'!' | b'='));
                    if method.is_empty() || singleton_or_op || method == "initialize" || private {
                        continue;
                    }
                    let Some(usage) = pending.take() else {
                        continue;
                    };
                    let name = if usage.replace('-', "_") == method {
                        usage
                    } else {
                        method
                    };
                    if valid_cli_name(name) {
                        let handler = CallQualifier::Attribute {
                            base: class.to_string(),
                            name: method.to_string(),
                        };
                        out.push(CliDecl::new(
                            CliFramework::Thor,
                            name.to_string(),
                            at_row(Some(handler), source, off),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    out
}

// ---- php: Symfony Console / Laravel artisan ---------------------------------

/// One `class Name ... { body }` of a PHP file.
struct PhpClass<'a> {
    name: &'a str,
    /// `[start, end)` of the body, inside its braces.
    body: (usize, usize),
    /// `extends` names a `*Command` (`Command`, `ContainerAwareCommand`,
    /// `\Illuminate\Console\Command`).
    console: bool,
}

/// Every named class of a PHP file, with its body and whether it extends a
/// console command. `Foo::class` and anonymous `new class` are not classes.
fn php_classes<'a>(source: &'a str, code: &CodeMap) -> Vec<PhpClass<'a>> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    for at in code_words(source, code, 0, source.len(), "class") {
        if at >= 2 && b[at - 2..at] == *b"::" {
            continue;
        }
        let (name, after) = read_ident(source, skip_ws(b, at + "class".len()));
        if name.is_empty() {
            continue;
        }
        let limit = b.len().min(after + MAX_ATTR_SPAN);
        let Some(open) = (after..limit).find(|&i| b[i] == b'{' && code.is_code(i)) else {
            continue;
        };
        let console = code_words(source, code, after, open, "extends")
            .first()
            .is_some_and(|&x| {
                let start = skip_ws(b, x + "extends".len());
                let mut end = start;
                while end < open && (is_ident_byte(b[end]) || b[end] == b'\\') {
                    end += 1;
                }
                source[start..end]
                    .rsplit('\\')
                    .next()
                    .is_some_and(|t| t.ends_with("Command"))
            });
        let body = balanced_code(source, code, open, MAX_CLASS_BODY).unwrap_or((open + 1, b.len()));
        out.push(PhpClass {
            name,
            body,
            console,
        });
    }
    out
}

/// The class a PHP attribute group closing at `from` (just past its `]`)
/// decorates: past further `#[...]` groups and `final` / `abstract` /
/// `readonly` to `class Name`.
fn php_attribute_target<'a>(source: &'a str, code: &CodeMap, from: usize) -> Option<&'a str> {
    let b = source.as_bytes();
    let mut i = from;
    for _ in 0..16 {
        i = skip_trivia_code(b, code, i);
        if b.get(i) == Some(&b'#') && b.get(i + 1) == Some(&b'[') {
            i = balanced_code(source, code, i + 1, MAX_ATTR_SPAN)?.1 + 1;
            continue;
        }
        let (word, after) = read_ident(source, i);
        match word {
            "final" | "abstract" | "readonly" => i = after,
            "class" => {
                let (name, _) = read_ident(source, skip_ws(b, after));
                return (!name.is_empty()).then_some(name);
            }
            _ => return None,
        }
    }
    None
}

/// The word before `at`, past whitespace (`static` in `protected static
/// $defaultName`, `string` in `?string $defaultName`).
fn php_prev_word(source: &str, at: usize) -> &str {
    let b = source.as_bytes();
    let mut end = at;
    while end > 0 && b[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    ident_before(source, end)
}

/// Symfony Console and Laravel artisan, each command handled by its class
/// (`Bare(Class)`). Symfony: `#[AsCommand(name: 'x')]` / `#[AsCommand('x')]`
/// on a class, and, in a class that extends a `*Command`, `static
/// $defaultName = 'x'` and `$this->setName('x')`; a `|` in a Symfony name
/// separates aliases, each a command. Laravel, in a class that extends a
/// `*Command`: `protected $signature = 'emails:send {user} {--queue}'` ->
/// `cli:emails:send` (the name ends at the first whitespace or `{`), and
/// `protected $name = 'x'`. A property is read only as a declaration (after
/// a visibility / `static` / type word), never as a local `$name = '...'`.
fn scan_symfony_laravel(source: &str, code: &CodeMap) -> Vec<CliDecl> {
    let b = source.as_bytes();
    let classes = php_classes(source, code);
    let mut found: Vec<(usize, CliDecl)> = Vec::new();
    let mut push =
        |at: usize, framework: CliFramework, names: &str, aliases: bool, class: Option<&str>| {
            let names: Vec<&str> = if aliases {
                names.split('|').collect()
            } else {
                vec![names]
            };
            for name in names.into_iter().filter(|n| valid_cli_name(n)) {
                let handler = class.map(|c| CallQualifier::Bare(c.to_string()));
                found.push((
                    at,
                    CliDecl::new(framework, name.to_string(), at_row(handler, source, at)),
                ));
            }
        };
    for at in code_words(source, code, 0, source.len(), "AsCommand") {
        let mut head = at;
        while head > 0 && (is_ident_byte(b[head - 1]) || b[head - 1] == b'\\') {
            head -= 1;
        }
        if head < 2 || b[head - 2..head] != *b"#[" {
            continue;
        }
        let Some(span) = call_args(source, code, at + "AsCommand".len()) else {
            continue;
        };
        let Some(names) = first_positional_str(source, span)
            .or_else(|| kwarg_str(source, code, span, "name", b':'))
        else {
            continue;
        };
        let class = balanced_code(source, code, head - 1, MAX_ATTR_SPAN)
            .and_then(|(_, close)| php_attribute_target(source, code, close + 1));
        push(at, CliFramework::Symfony, &names, true, class);
    }
    for class in classes.iter().filter(|c| c.console) {
        let (start, end) = class.body;
        for (var, framework) in [
            ("$defaultName", CliFramework::Symfony),
            ("$signature", CliFramework::Laravel),
            ("$name", CliFramework::Laravel),
        ] {
            for at in code_words(source, code, start, end, var) {
                if !matches!(
                    php_prev_word(source, at),
                    "public" | "protected" | "private" | "static" | "var" | "string"
                ) {
                    continue;
                }
                let eq = skip_ws(b, at + var.len());
                if b.get(eq) != Some(&b'=') || matches!(b.get(eq + 1), Some(b'=' | b'>')) {
                    continue;
                }
                let Some(lit) = source.get(skip_ws(b, eq + 1)..end).and_then(quoted_literal) else {
                    continue;
                };
                let name = if var == "$signature" {
                    let lit = lit.trim_start();
                    &lit[..lit
                        .find(|c: char| c.is_whitespace() || c == '{')
                        .unwrap_or(lit.len())]
                } else {
                    lit
                };
                push(at, framework, name, var == "$defaultName", Some(class.name));
            }
        }
        for at in code_words(source, code, start, end, "setName") {
            if source[..at].ends_with("$this->")
                && let Some(span) = call_args(source, code, at + "setName".len())
                && let Some(name) = first_positional_str(source, span)
            {
                push(at, CliFramework::Symfony, &name, true, Some(class.name));
            }
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found.into_iter().map(|(_, decl)| decl).collect()
}

// ----------------------------------------------------------------------------
// DG.8: scopt (scala). Scala's literals and comments are the C family's
// (`"..."`, `"""..."""`, `'c'`, `//`, nesting `/* */`), so a scopt file is
// lexed by [`CodeMap::new`].
// ----------------------------------------------------------------------------

/// scopt, in a Scala file that mentions `scopt`: the scopt 4 builder's
/// `programName("x")` (the binary) and `cmd("y")` (a subcommand, at any
/// `children(..)` depth), and scopt 3's `OptionParser[T]("x")` constructor
/// (`new scopt.OptionParser[Config]("x") { cmd("y") .. }`, its type argument
/// stepped over) -> `cli:x` / `cli:y`, in source order. Bare (under
/// `import builder._`) or qualified (`builder.cmd("y")`). The argument must
/// be exactly one plain string literal: an interpolation (`cmd(s"..")`) or an
/// expression names nothing, and a needle in a string or comment is not a
/// call. No handler: a scopt `.action((_, c) => ..)` is a lambda updating the
/// config, not a symbol (LA.20a: a ref only when the declaration names the
/// code). `opt[T]("x")`, `arg`, `head` and `note` declare options and help
/// text, not commands, and are not read.
fn scan_scopt(source: &str, code: &CodeMap) -> Vec<CliDecl> {
    let b = source.as_bytes();
    let mut found: Vec<(usize, String)> = Vec::new();
    for word in ["programName", "cmd", "OptionParser"] {
        for at in code_words(source, code, 0, source.len(), word) {
            let mut i = skip_ws(b, at + word.len());
            if word == "OptionParser" {
                if b.get(i) != Some(&b'[') {
                    continue;
                }
                let Some((_, close)) = balanced_code(source, code, i, MAX_ATTR_SPAN) else {
                    continue;
                };
                i = close + 1;
            }
            let Some(span) = call_args(source, code, i) else {
                continue;
            };
            if let Some(name) = first_positional_str(source, span).filter(|n| valid_cli_name(n)) {
                found.push((at, name));
            }
        }
    }
    found.sort_by_key(|(at, _)| *at);
    found
        .into_iter()
        .map(|(_, name)| CliDecl::new(CliFramework::Scopt, name, None))
        .collect()
}

// ----------------------------------------------------------------------------
// DH.10: Dart's package:args and Elixir's Mix tasks. Both files are lexed by
// [`CodeMap::script`] ([`Script::Dart`] / [`Script::Elixir`]), so a needle in
// a string, a doc heredoc or a comment declares nothing.
// ----------------------------------------------------------------------------

/// One `class Name ... { body }` of a Dart file.
struct DartClass<'a> {
    name: &'a str,
    /// The superclass's last name segment, type arguments dropped
    /// (`args.Command<void>` -> `Command`); empty without `extends`.
    base: &'a str,
    /// `[start, end)` of the body, inside its braces.
    body: (usize, usize),
}

/// Every named class of a Dart file with a body (`abstract class`, `final
/// class` and the rest included). A mixin application `class A = B with M;`
/// has no body and is passed over: its `;` comes before any `{`.
fn dart_classes<'a>(source: &'a str, code: &CodeMap) -> Vec<DartClass<'a>> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    for at in code_words(source, code, 0, source.len(), "class") {
        let (name, after) = read_ident(source, skip_ws(b, at + "class".len()));
        if name.is_empty() {
            continue;
        }
        let limit = b.len().min(after + MAX_ATTR_SPAN);
        let Some(open) = (after..limit).find(|&i| matches!(b[i], b'{' | b';') && code.is_code(i))
        else {
            continue;
        };
        if b[open] != b'{' {
            continue;
        }
        let base = code_words(source, code, after, open, "extends")
            .first()
            .map_or("", |&x| {
                let mut i = skip_ws(b, x + "extends".len());
                let mut last = "";
                loop {
                    let (seg, end) = read_ident(source, i);
                    if seg.is_empty() {
                        break;
                    }
                    last = seg;
                    if b.get(end) != Some(&b'.') {
                        break;
                    }
                    i = end + 1;
                }
                last
            });
        let body = balanced_code(source, code, open, MAX_CLASS_BODY).unwrap_or((open + 1, b.len()));
        out.push(DartClass { name, base, body });
    }
    out
}

/// Index just past the Dart type-argument list opening at `open` (`<void>`,
/// `<Map<String, int>>`), or `None` when it does not close before a `;`, a
/// brace or [`MAX_ATTR_SPAN`] bytes.
fn dart_type_args_end(b: &[u8], open: usize) -> Option<usize> {
    let limit = b.len().min(open.saturating_add(MAX_ATTR_SPAN));
    let mut depth = 0usize;
    for (i, &c) in b.iter().enumerate().take(limit).skip(open) {
        match c {
            b'<' => depth += 1,
            b'>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            b';' | b'{' | b'}' => return None,
            _ => {}
        }
    }
    None
}

/// The command name a `Command` subclass body declares at its top level (not
/// inside a method, a parameter list or a collection), with its offset:
/// `final name = 'x';`, `final String name = 'x';`, `String name = 'x';`,
/// `var name = 'x';`, `String get name => 'x';` or `String get name {
/// return 'x'; }`. The value must be exactly one plain string literal ending
/// the statement: an interpolation (`'$prefix-sync'`), a concatenation or a
/// variable names nothing.
fn dart_command_name(
    source: &str,
    code: &CodeMap,
    (start, end): (usize, usize),
) -> Option<(usize, String)> {
    let b = source.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < end {
        let next = code.next_code(i);
        if next != i {
            i = next;
            continue;
        }
        match b[i] {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth = depth.saturating_sub(1),
            b'n' if depth == 0
                && b[i..].starts_with(b"name")
                && !(i > 0 && (is_ident_byte(b[i - 1]) || matches!(b[i - 1], b'$' | b'.')))
                && !b.get(i + 4).copied().is_some_and(is_ident_byte) =>
            {
                if let Some(name) = dart_name_value(source, i, end) {
                    return Some((i, name));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The literal a `name` member at `at` is declared with (see
/// [`dart_command_name`]), read up to `end`.
fn dart_name_value(source: &str, at: usize, end: usize) -> Option<String> {
    let b = source.as_bytes();
    let after = skip_ws(b, at + "name".len());
    let value = match php_prev_word(source, at) {
        "get" if b[after..].starts_with(b"=>") => skip_ws(b, after + 2),
        "get" if b.get(after) == Some(&b'{') => {
            let (word, ret) = read_ident(source, skip_ws(b, after + 1));
            if word != "return" {
                return None;
            }
            skip_ws(b, ret)
        }
        "final" | "String" | "var"
            if b.get(after) == Some(&b'=') && !matches!(b.get(after + 1), Some(b'=' | b'>')) =>
        {
            skip_ws(b, after + 1)
        }
        _ => return None,
    };
    let lit = source.get(value..end).and_then(quoted_literal)?;
    let close = skip_ws(b, value + lit.len() + 2);
    (b.get(close) == Some(&b';') && valid_cli_name(lit)).then(|| lit.to_string())
}

/// package:args (pub.dev/packages/args), in a Dart file that names
/// `package:args/`: the executable a `CommandRunner` is constructed with
/// (`CommandRunner('mytool', 'desc')`, `CommandRunner<void>(..)`, `new
/// CommandRunner(..)`, or a subclass's `: super('mytool', ..)`: its
/// documented `executableName`) -> `cli:mytool`, no handler; and each class
/// extending `Command` (or a `*Command` base, `Command<T>` included) whose
/// body declares its `name` ([`dart_command_name`]) -> `cli:<name>` handled
/// by `Bare(<Class>)`, the Symfony precedent (a command is its class; its
/// `run` shares a name with every other command's). In source order. An
/// abstract base with no literal `name` declares nothing. GD.21: a named
/// command's top-level `aliases` ([`dart_command_aliases`]) are commands
/// too, each handled by the same class (package:args runs a command by any
/// alias); the second value counts them.
fn scan_dart_args(source: &str, code: &CodeMap) -> (Vec<CliDecl>, usize) {
    let b = source.as_bytes();
    let mut aliases = 0usize;
    let first_str = |at: usize| {
        call_args(source, code, at)
            .and_then(|span| first_positional_str(source, span))
            .filter(|n| valid_cli_name(n))
    };
    let mut found: Vec<(usize, CliDecl)> = Vec::new();
    for at in code_words(source, code, 0, source.len(), "CommandRunner") {
        let mut i = skip_ws(b, at + "CommandRunner".len());
        if b.get(i) == Some(&b'<') {
            let Some(close) = dart_type_args_end(b, i) else {
                continue;
            };
            i = close;
        }
        if let Some(name) = first_str(i) {
            found.push((at, CliDecl::new(CliFramework::CommandRunner, name, None)));
        }
    }
    for class in dart_classes(source, code) {
        let (start, end) = class.body;
        if class.base == "CommandRunner" {
            if let Some((at, name)) = code_words(source, code, start, end, "super")
                .into_iter()
                .find_map(|at| first_str(at + "super".len()).map(|name| (at, name)))
            {
                found.push((at, CliDecl::new(CliFramework::CommandRunner, name, None)));
            }
        } else if class.base.ends_with("Command")
            && let Some((at, name)) = dart_command_name(source, code, class.body)
        {
            let handler = CallQualifier::Bare(class.name.to_string());
            for (alias_at, alias) in dart_command_aliases(source, code, class.body) {
                aliases += 1;
                found.push((
                    alias_at,
                    CliDecl::new(
                        CliFramework::CommandRunner,
                        alias,
                        at_row(Some(handler.clone()), source, alias_at),
                    ),
                ));
            }
            found.push((
                at,
                CliDecl::new(
                    CliFramework::CommandRunner,
                    name,
                    at_row(Some(handler), source, at),
                ),
            ));
        }
    }
    // Stable: a command's aliases share one offset and keep their order.
    found.sort_by_key(|(at, _)| *at);
    (found.into_iter().map(|(_, decl)| decl).collect(), aliases)
}

/// GD.21: the aliases a `Command` subclass body declares at its top level
/// (as [`dart_command_name`] reads `name`), each with the `aliases` offset:
/// `List<String> get aliases => ['a', 'b'];`, `get aliases { return [..];
/// }`, or a field `final aliases = const <String>['a'];` (any declared
/// type). Each plain string literal of the list is an alias; an
/// interpolation or a variable element is passed over, and a list built
/// any other way (a call, a spread, a variable) names nothing.
fn dart_command_aliases(
    source: &str,
    code: &CodeMap,
    (start, end): (usize, usize),
) -> Vec<(usize, String)> {
    let b = source.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < end {
        let next = code.next_code(i);
        if next != i {
            i = next;
            continue;
        }
        match b[i] {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth = depth.saturating_sub(1),
            b'a' if depth == 0
                && b[i..].starts_with(b"aliases")
                && !(i > 0 && (is_ident_byte(b[i - 1]) || matches!(b[i - 1], b'$' | b'.')))
                && !b.get(i + 7).copied().is_some_and(is_ident_byte) =>
            {
                if let Some(names) = dart_aliases_value(source, code, i, end) {
                    return names.into_iter().map(|n| (i, n)).collect();
                }
            }
            _ => {}
        }
        i += 1;
    }
    Vec::new()
}

/// The aliases an `aliases` member at `at` is declared with (see
/// [`dart_command_aliases`]), read up to `end`.
fn dart_aliases_value(source: &str, code: &CodeMap, at: usize, end: usize) -> Option<Vec<String>> {
    let b = source.as_bytes();
    let after = skip_ws(b, at + "aliases".len());
    let getter = php_prev_word(source, at) == "get";
    let mut v = if getter && b[after..].starts_with(b"=>") {
        skip_ws(b, after + 2)
    } else if getter && b.get(after) == Some(&b'{') {
        let (word, ret) = read_ident(source, skip_ws(b, after + 1));
        if word != "return" {
            return None;
        }
        skip_ws(b, ret)
    } else if !getter
        && b.get(after) == Some(&b'=')
        && !matches!(b.get(after + 1), Some(b'=' | b'>'))
    {
        skip_ws(b, after + 1)
    } else {
        return None;
    };
    let (word, word_end) = read_ident(source, v);
    if word == "const" {
        v = skip_ws(b, word_end);
    }
    if b.get(v) == Some(&b'<') {
        v = skip_ws(b, dart_type_args_end(b, v)?);
    }
    if b.get(v) != Some(&b'[') {
        return None;
    }
    let (list_start, list_end) = balanced_code(source, code, v, MAX_ATTR_SPAN)?;
    if list_end > end || b.get(skip_ws(b, list_end + 1)) != Some(&b';') {
        return None;
    }
    let names = source[list_start..list_end]
        .split(',')
        .map(str::trim)
        .filter_map(|item| {
            let lit = quoted_literal(item)?;
            (lit.len() + 2 == item.len() && valid_cli_name(lit)).then(|| lit.to_string())
        })
        .collect();
    Some(names)
}

/// The Elixir alias at `i` (`Mix.Tasks.Shop.Sync`: identifier bytes and
/// dots, starting upper-case, a trailing `.` dropped) and the index after it.
fn elixir_alias(source: &str, i: usize) -> (&str, usize) {
    let b = source.as_bytes();
    if !b.get(i).is_some_and(u8::is_ascii_uppercase) {
        return ("", i);
    }
    let mut end = i;
    while b.get(end).is_some_and(|&c| is_ident_byte(c) || c == b'.') {
        end += 1;
    }
    while end > i && b[end - 1] == b'.' {
        end -= 1;
    }
    (&source[i..end], end)
}

/// Elixir's `Macro.underscore` on one alias segment, the rule Mix's task
/// names follow: `ShopSync` -> `shop_sync`, `HTTPServer` -> `http_server`,
/// `Foo2Bar` -> `foo2_bar`. An upper-case letter followed by a lower-case
/// one starts a word; so does one after a non-upper-case byte other than `_`.
fn macro_underscore(seg: &str) -> String {
    let b = seg.as_bytes();
    let mut out = String::with_capacity(b.len() + 4);
    let Some(&first) = b.first() else { return out };
    out.push(char::from(first.to_ascii_lowercase()));
    let (mut prev, mut i) = (first, 1);
    while i < b.len() {
        let h = b[i];
        if h.is_ascii_uppercase() {
            if let Some(&t) = b.get(i + 1)
                && !t.is_ascii_uppercase()
                && !t.is_ascii_digit()
                && t != b'_'
            {
                out.push('_');
                out.push(char::from(h.to_ascii_lowercase()));
                out.push(char::from(t));
                (prev, i) = (t, i + 2);
                continue;
            }
            if !prev.is_ascii_uppercase() && prev != b'_' {
                out.push('_');
            }
        }
        out.push(char::from(h.to_ascii_lowercase()));
        (prev, i) = (h, i + 1);
    }
    out
}

/// True when the `use` at `at` is `use Mix.Task` or `use Mix.Task.Compiler`
/// (a compiler is a task too, `Mix.Tasks.Compile.X` run as `mix compile.x`).
fn uses_mix_task(source: &str, at: usize) -> bool {
    let b = source.as_bytes();
    let i = at + "use".len();
    if !matches!(b.get(i), Some(b' ' | b'\t')) {
        return false;
    }
    matches!(
        elixir_alias(source, skip_ws(b, i)).0,
        "Mix.Task" | "Mix.Task.Compiler"
    )
}

/// Mix tasks, in an Elixir file that names `Mix.Task`: each `defmodule
/// Mix.Tasks.<Seg>(.<Seg>)*` whose module uses `Mix.Task` (a code `use
/// Mix.Task` after the `defmodule` and before the file's next one) ->
/// `cli:<task>`, the task name Mix derives (`Mix.Task.task_name/1`: the
/// segments after `Mix.Tasks.`, each [`macro_underscore`]d, joined with
/// `.`), so `Mix.Tasks.Shop.Sync` is `cli:shop.sync` (`mix shop.sync`) and
/// `Mix.Tasks.Ecto.Gen.Repo` is `cli:ecto.gen.repo`. Handled by the task
/// module, `Bare(<last segment>)` (the name the Elixir parser gives the
/// module's PACKAGE): its `run/1` is the entry point, but every task has one,
/// and the bare `run` would be ambiguous where the module is unique. A
/// `Mix.Tasks.*` module without `use Mix.Task` (a helper) is not a task.
fn scan_mix_tasks(source: &str, code: &CodeMap) -> Vec<CliDecl> {
    let b = source.as_bytes();
    let defs = code_words(source, code, 0, source.len(), "defmodule");
    let uses: Vec<usize> = code_words(source, code, 0, source.len(), "use")
        .into_iter()
        .filter(|&at| uses_mix_task(source, at))
        .collect();
    let mut out = Vec::new();
    for (k, &at) in defs.iter().enumerate() {
        let next = defs.get(k + 1).copied().unwrap_or(b.len());
        if !uses.iter().any(|&u| u > at && u < next) {
            continue;
        }
        let (alias, after) = elixir_alias(source, skip_ws(b, at + "defmodule".len()));
        let Some(rest) = alias.strip_prefix("Mix.Tasks.") else {
            continue;
        };
        if !matches!(b.get(after), Some(b' ' | b'\t' | b'\r' | b'\n' | b',')) {
            continue;
        }
        let segs: Vec<&str> = rest.split('.').collect();
        if !segs
            .iter()
            .all(|s| s.starts_with(|c: char| c.is_ascii_uppercase()))
        {
            continue;
        }
        let name = segs
            .iter()
            .map(|s| macro_underscore(s))
            .collect::<Vec<_>>()
            .join(".");
        if !valid_cli_name(&name) {
            continue;
        }
        let handler = segs.last().map(|s| CallQualifier::Bare((*s).to_string()));
        out.push(CliDecl::new(
            CliFramework::MixTask,
            name,
            at_row(handler, source, at),
        ));
    }
    out
}

// ----------------------------------------------------------------------------
// LA.20c: binaries declared by packaging manifests. A CLI library declares the
// SUBCOMMAND (`@click.command("sync")`); the BINARY a caller's argv[0] names
// (`mytool`) is declared only by the package's manifest, so without these
// scans `cli_invoke:mytool` has no `cli:mytool` to pair with. Line scanners
// (the crate has no TOML dependency), deliberately narrow: the three Python
// script tables, Cargo's `[[bin]]` name, npm's `bin`, setuptools'
// `console_scripts`, and (DH.10) composer.json's `bin` and a gemspec's
// `executables`. No HANDLED_BY ref here: a manifest is a synthetic parse in
// the `manifest` language group, and refs never resolve across language
// graphs. GH.20: each binary instead carries the target its manifest names
// (a [`manifest_bin_cell`]), which `resolver:host` binds across graphs.
// ----------------------------------------------------------------------------

/// The manifest form that declared a binary: the `[cli-decl] manifest`
/// marker's counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManifestForm {
    Pyproject,
    SetupPy,
    Npm,
    Cargo,
    /// DH.10: composer.json `bin`.
    Composer,
    /// DH.10: a `*.gemspec`'s `executables`.
    Gemspec,
}

impl ManifestForm {
    /// GH.20: the form's word in a [`manifest_bin_cell`] and the
    /// `[host-start]` marker.
    fn label(self) -> &'static str {
        match self {
            ManifestForm::Pyproject => "pyproject",
            ManifestForm::SetupPy => "setup",
            ManifestForm::Npm => "npm",
            ManifestForm::Cargo => "cargo",
            ManifestForm::Composer => "composer",
            ManifestForm::Gemspec => "gemspec",
        }
    }

    /// The form a manifest path's basename names. Never [`ManifestForm::SetupPy`]:
    /// setup.py is Python source, routed to the python arm of
    /// [`extract_cli_command_nodes`], not to the manifest branch.
    fn of_path(path: &str) -> Option<Self> {
        match path.rsplit('/').next().unwrap_or(path) {
            "pyproject.toml" => Some(ManifestForm::Pyproject),
            "package.json" => Some(ManifestForm::Npm),
            "Cargo.toml" => Some(ManifestForm::Cargo),
            "composer.json" => Some(ManifestForm::Composer),
            base if crate::packages::is_gemspec_name(base) => Some(ManifestForm::Gemspec),
            _ => None,
        }
    }
}

/// Longest `console_scripts` list (or `entry_points` string) the setup.py scan reads.
const MAX_SCRIPTS_SPAN: usize = 16 * 1024;

/// GH.20: one binary a manifest declares, the target it names (a Cargo /
/// npm / composer / gemspec path as written, or Cargo's default; a Python
/// `pkg.mod:attr` object reference) and the 0-based line that declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestBin {
    name: String,
    target: Option<String>,
    line: u32,
}

impl ManifestBin {
    fn new(name: impl Into<String>, target: Option<String>, line: u32) -> Self {
        ManifestBin {
            name: name.into(),
            target: target.filter(|t| !t.is_empty()),
            line,
        }
    }
}

/// The [`manifest_bin_cell`] JSON shape. Field order is the serialised
/// order, so the payload is byte-stable.
#[derive(serde::Serialize, serde::Deserialize)]
struct ManifestBinPayload<'a> {
    #[serde(borrow)]
    manifest: std::borrow::Cow<'a, str>,
    #[serde(borrow, default, skip_serializing_if = "Option::is_none")]
    file: Option<std::borrow::Cow<'a, str>>,
    line: u32,
    #[serde(borrow)]
    target: std::borrow::Cow<'a, str>,
}

/// GH.20: what one [`manifest_bin_cell`] says about a manifest-declared
/// `cli:<bin>` CLI_COMMAND (read by [`parse_manifest_bin_cell`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ManifestBinTarget {
    /// The manifest form: `cargo`, `pyproject`, `setup`, `npm`, `gemspec`
    /// or `composer`.
    pub manifest: String,
    /// The repo-relative manifest path; `None` for a setup.py binary, whose
    /// CLI_COMMAND's parent is the (located) setup.py MODULE itself.
    pub file: Option<String>,
    /// The 0-based line of the declaration in the manifest.
    pub line: u32,
    /// The target as the manifest names it, relative to the manifest's
    /// directory: a file path (cargo, npm, composer, gemspec) or a Python
    /// object reference `pkg.mod[:attr[.sub]]` (pyproject, setup).
    pub target: String,
}

/// GH.20: the CODE cell a manifest-declared CLI_COMMAND carries, e.g.
/// `{"manifest":"cargo","file":"svc/Cargo.toml","line":7,"target":"src/bin/api.rs"}`
/// ([`ManifestBinTarget`] names the fields). The sibling of [`argv_cell`]:
/// written only here and read only by [`parse_manifest_bin_cell`], so the
/// format lives in one place; neither reader parses the other's payload.
pub fn manifest_bin_cell(manifest: &str, file: Option<&str>, line: u32, target: &str) -> Cell {
    let payload = ManifestBinPayload {
        manifest: std::borrow::Cow::Borrowed(manifest),
        file: file.map(std::borrow::Cow::Borrowed),
        line,
        target: std::borrow::Cow::Borrowed(target),
    };
    Cell {
        kind: cell_type::CODE,
        payload: CellPayload::Json(serde_json::to_string(&payload).unwrap_or_default()),
    }
}

/// The [`ManifestBinTarget`] of one [`manifest_bin_cell`]; `None` for any
/// other cell (an [`argv_cell`] included) or a payload that does not parse.
pub fn parse_manifest_bin_cell(cell: &Cell) -> Option<ManifestBinTarget> {
    let CellPayload::Json(json) = &cell.payload else {
        return None;
    };
    if cell.kind != cell_type::CODE {
        return None;
    }
    let p = serde_json::from_str::<ManifestBinPayload>(json).ok()?;
    Some(ManifestBinTarget {
        manifest: p.manifest.into_owned(),
        file: p.file.map(std::borrow::Cow::into_owned),
        line: p.line,
        target: p.target.into_owned(),
    })
}

/// LA.20c: every binary a packaging manifest declares, as a flat `cli:<bin>`
/// CLI_COMMAND parented to the manifest's MODULE, so it is located at the
/// manifest and `CliInvocationResolver` pairs `cli_invoke:<bin>` with it.
/// Dispatch is by basename: pyproject.toml `[project.scripts]` /
/// `[project.gui-scripts]` / `[tool.poetry.scripts]` keys, package.json `bin`
/// (its keys, or the unscoped package `name` when `bin` is one path), each
/// Cargo.toml `[[bin]]` table's `name`, and (DH.10) the basename of each
/// composer.json `bin` path and every name a `*.gemspec` assigns to
/// `executables`. A Cargo package with no `[[bin]]` is not assumed to be a
/// binary: a per-file extractor cannot see whether `src/main.rs` exists. Any
/// other path yields nothing.
pub fn extract_manifest_binaries(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> CliNodes {
    let mut residuals = Residuals::default();
    let form = ManifestForm::of_path(path);
    let bins = match form {
        Some(ManifestForm::Pyproject) => scan_pyproject_scripts(source),
        Some(ManifestForm::Npm) => scan_package_json_bin(source),
        Some(ManifestForm::Cargo) => scan_cargo_bins(source),
        Some(ManifestForm::Composer) => scan_composer_bin(source),
        Some(ManifestForm::Gemspec) => {
            let (bins, concat) = scan_gemspec_executables(source);
            residuals.gemspec_concat = concat;
            bins
        }
        Some(ManifestForm::SetupPy) | None => Vec::new(),
    };
    // GD.21 fired_on: the gemspec's `concat` / `+=` names.
    if let Some(line) = residuals.marker(path) {
        eprintln!("{line}");
    }
    let mut out = CliNodes::default();
    let mut seen = std::collections::HashSet::new();
    for bin in bins {
        if is_bin_name(&bin.name) && seen.insert(bin.name.clone()) {
            add_cli_command(&mut out.nodes, &mut out.nav, &bin.name, module_id, repo);
            // GH.20: the target rides on the command for resolver:host.
            if let (Some(form), Some(target), Some(node)) =
                (form, bin.target.as_deref(), out.nodes.last_mut())
            {
                node.cells.push(manifest_bin_cell(
                    form.label(),
                    Some(path),
                    bin.line,
                    target,
                ));
            }
        }
    }
    if !out.nodes.is_empty() {
        out.per_framework
            .push((CliFramework::Manifest, out.nodes.len()));
    }
    out
}

/// LA.20c fired_on, one per manifest that declared a binary:
/// `[cli-decl] manifest bins=1 (pyproject=1 setup=0 npm=0 cargo=0 composer=0
/// gemspec=0) path=pyproject.toml` (DH.10 added the last two counters).
/// `None` when `out` holds no manifest binary or `path` is not a manifest.
pub fn manifest_marker(path: &str, out: &CliNodes) -> Option<String> {
    let form = ManifestForm::of_path(path)?;
    let n = out
        .per_framework
        .iter()
        .find(|(f, _)| *f == CliFramework::Manifest)
        .map_or(0, |(_, n)| *n);
    (n > 0).then(|| manifest_line(form, n, path))
}

fn manifest_line(form: ManifestForm, n: usize, path: &str) -> String {
    let count = |f: ManifestForm| if f == form { n } else { 0 };
    format!(
        "[cli-decl] manifest bins={n} (pyproject={} setup={} npm={} cargo={} composer={} gemspec={}) path={path}",
        count(ManifestForm::Pyproject),
        count(ManifestForm::SetupPy),
        count(ManifestForm::Npm),
        count(ManifestForm::Cargo),
        count(ManifestForm::Composer),
        count(ManifestForm::Gemspec),
    )
}

/// A binary name a manifest declares: `[A-Za-z0-9._-]+`, at most 64 bytes, not
/// starting with `.` or `-`. A scoped npm package name (`@acme/x`) is not one.
fn is_bin_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['.', '-'])
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// The dotted name of a TOML `[table]` header line, key segments trimmed and
/// unquoted (`[ project . "scripts" ]` -> `project.scripts`). `None` for an
/// `[[array-of-tables]]` header or a line that is not a header.
fn toml_table_header(line: &str) -> Option<String> {
    let inner = line.strip_prefix('[').filter(|r| !r.starts_with('['))?;
    let (inner, _) = inner.split_once(']')?;
    let segs: Vec<&str> = inner
        .split('.')
        .map(|seg| seg.trim().trim_matches(|c| c == '"' || c == '\''))
        .collect();
    Some(segs.join("."))
}

/// `key = value` on one trimmed TOML line: the key (bare `[A-Za-z0-9._-]+`, or
/// quoted and returned raw) and the value text after `=`. `None` for a blank
/// line, a comment, a header, or a key with no value.
fn toml_key_value(line: &str) -> Option<(&str, &str)> {
    let (key, rest) = if line.starts_with(['"', '\'']) {
        let key = quoted_literal(line)?;
        (key, line.get(key.len() + 2..)?)
    } else {
        let end = line
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
            .unwrap_or(line.len());
        line.split_at(end)
    };
    let value = rest.trim_start().strip_prefix('=')?.trim_start();
    (!key.is_empty() && !value.is_empty() && !value.starts_with('=')).then_some((key, value))
}

/// The `{` nesting depth after `text`, starting from `depth`: braces inside a
/// string or after a `#` comment do not count. How the pyproject scan steps
/// over the continuation lines of a multi-line inline table.
fn inline_table_depth(text: &str, mut depth: usize) -> usize {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in text.chars() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' && q == '"' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '#' => break,
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                _ => {}
            },
        }
    }
    depth
}

/// pyproject.toml: the key of every entry of `[project.scripts]`,
/// `[project.gui-scripts]` and `[tool.poetry.scripts]`, up to the next header.
/// A poetry table value (`x = { reference = "m:f", type = "console" }`) still
/// names the binary by its key; the continuation lines of a multi-line inline
/// table are stepped over, never read as keys. GH.20: the target is the
/// string value (`"pkg.mod:attr"`), or an inline table's `callable` /
/// `reference` string unless its `type` is `"file"` (a script path, not an
/// object reference).
fn scan_pyproject_scripts(source: &str) -> Vec<ManifestBin> {
    let mut out = Vec::new();
    let mut in_scripts = false;
    let mut depth = 0usize;
    for (row, line) in (0u32..).zip(source.lines()) {
        let t = line.trim();
        if depth > 0 {
            depth = inline_table_depth(t, depth);
            continue;
        }
        if t.starts_with('[') {
            in_scripts = matches!(
                toml_table_header(t).as_deref(),
                Some("project.scripts" | "project.gui-scripts" | "tool.poetry.scripts")
            );
            continue;
        }
        if !in_scripts {
            continue;
        }
        if let Some((key, value)) = toml_key_value(t) {
            depth = inline_table_depth(value, 0);
            out.push(ManifestBin::new(key, pyproject_script_target(value), row));
        }
    }
    out
}

/// GH.20: the object reference a pyproject script entry's value names (see
/// [`scan_pyproject_scripts`]), only off the entry's own line.
fn pyproject_script_target(value: &str) -> Option<String> {
    if let Some(table) = value.strip_prefix('{') {
        if inline_table_string(table, "type").as_deref() == Some("file") {
            return None;
        }
        return inline_table_string(table, "callable")
            .or_else(|| inline_table_string(table, "reference"));
    }
    quoted_literal(value).map(str::to_string)
}

/// The string value of `key = "..."` inside one line of an inline table's
/// text (`reference = "m:f", type = "console" }`).
fn inline_table_string(table: &str, key: &str) -> Option<String> {
    table.split(',').find_map(|part| {
        let (k, v) = toml_key_value(part.trim())?;
        (k == key)
            .then(|| quoted_literal(v.trim_end_matches('}').trim()))?
            .map(str::to_string)
    })
}

/// Cargo.toml: the `name` of each `[[bin]]` table. GH.20: the target is the
/// table's `path`, else Cargo's default: `src/main.rs` for the bin named
/// like the `[package]`, `src/bin/<name>.rs` for any other (resolver:host
/// also tries `src/bin/<name>/main.rs`).
fn scan_cargo_bins(source: &str) -> Vec<ManifestBin> {
    // One `[[bin]]` table: its `name` (with the line) and its `path`.
    #[derive(Default)]
    struct Table {
        name: Option<(String, u32)>,
        path: Option<String>,
    }
    let mut tables: Vec<Table> = Vec::new();
    let mut package: Option<String> = None;
    let mut section = "";
    for (row, line) in (0u32..).zip(source.lines()) {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("[[") {
            let is_bin = rest
                .split_once("]]")
                .is_some_and(|(name, _)| name.trim() == "bin");
            section = if is_bin { "bin" } else { "" };
            if is_bin {
                tables.push(Table::default());
            }
            continue;
        }
        if t.starts_with('[') {
            section = if toml_table_header(t).as_deref() == Some("package") {
                "package"
            } else {
                ""
            };
            continue;
        }
        let Some((key, value)) = toml_key_value(t) else {
            continue;
        };
        match (section, key) {
            ("package", "name") => package = extract_quoted(value),
            ("bin", "name") => {
                if let (Some(slot), Some(name)) = (tables.last_mut(), extract_quoted(value)) {
                    slot.name = Some((name, row));
                }
            }
            ("bin", "path") => {
                if let (Some(slot), Some(path)) = (tables.last_mut(), quoted_literal(value)) {
                    slot.path = Some(path.to_string());
                }
            }
            _ => {}
        }
    }
    tables
        .into_iter()
        .filter_map(|Table { name, path }| {
            let (name, row) = name?;
            let target = path.unwrap_or_else(|| {
                if package.as_deref() == Some(name.as_str()) {
                    "src/main.rs".to_string()
                } else {
                    format!("src/bin/{name}.rs")
                }
            });
            Some(ManifestBin::new(name, Some(target), row))
        })
        .collect()
}

/// package.json: `bin` as an object names each binary by its key; `bin` as one
/// path names it after the package, `@scope/` removed. GH.20: the target is
/// the path each names.
fn scan_package_json_bin(source: &str) -> Vec<ManifestBin> {
    use serde_json::Value;
    let Ok(Value::Object(pkg)) = serde_json::from_str::<Value>(source) else {
        return Vec::new();
    };
    match pkg.get("bin") {
        Some(Value::Object(bins)) => bins
            .iter()
            .map(|(name, path)| {
                let path = path.as_str().map(str::to_string);
                let row = json_line(source, path.as_deref().unwrap_or(name));
                ManifestBin::new(name.clone(), path, row)
            })
            .collect(),
        Some(Value::String(path)) => pkg
            .get("name")
            .and_then(Value::as_str)
            .map(|name| {
                let bare = name
                    .strip_prefix('@')
                    .and_then(|scoped| scoped.split_once('/'))
                    .map_or(name, |(_, bare)| bare);
                ManifestBin::new(bare, Some(path.clone()), json_line(source, path))
            })
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// GH.20: the 0-based line of the first `"<text>"` string after the manifest's
/// `"bin"` key (the line of `"bin"` when there is none, 0 with no `"bin"`).
fn json_line(source: &str, text: &str) -> u32 {
    let Some(bin) = source.find("\"bin\"") else {
        return 0;
    };
    let needle = format!("\"{text}\"");
    let at = source[bin..].find(&needle).map_or(bin, |i| bin + i);
    line_of(source, at)
}

/// DH.10 composer.json: `bin` is a list of paths (or one path), each
/// installed under `vendor/bin/` by its basename (`bin/mytool` -> `mytool`).
/// GH.20: the target is the path itself.
fn scan_composer_bin(source: &str) -> Vec<ManifestBin> {
    use serde_json::Value;
    let Ok(Value::Object(pkg)) = serde_json::from_str::<Value>(source) else {
        return Vec::new();
    };
    let paths: Vec<&str> = match pkg.get("bin") {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(one)) => vec![one.as_str()],
        _ => Vec::new(),
    };
    paths
        .into_iter()
        .filter_map(|p| {
            let name = p.rsplit(['/', '\\']).next()?;
            Some(ManifestBin::new(
                name,
                Some(p.to_string()),
                json_line(source, p),
            ))
        })
        .collect()
}

/// DH.10 `*.gemspec`: every name assigned to a spec's executables, read off
/// a Ruby-lexed source ([`CodeMap::script`], so `#` comments and strings
/// declare nothing): `<spec>.executables = ["a", 'b']` (a list may span
/// lines; non-literal elements are passed over), `= %w[a b]` (any `%w` /
/// `%W` delimiter), `= "a"`, `<< "a"`, and the singular `<spec>.executable =
/// "a"`; GD.21: `+= [..]` / `+= %w[..]` and `.concat([..])` / `.concat(%w[..])`
/// (or paren-less `.concat %w[..]`), whose names the second value counts. A
/// computed list (`spec.files.grep(%r{^exe/}) { .. }`, `Dir[..]`) names no
/// literal binary and reads nothing. In source order. GH.20: each target is
/// `<bindir>/<name>`, the spec's literal `bindir = "exe"` or RubyGems'
/// default `bin`.
fn scan_gemspec_executables(source: &str) -> (Vec<ManifestBin>, usize) {
    let b = source.as_bytes();
    let code = CodeMap::script(source, Script::Ruby);
    let mut found: Vec<(usize, String)> = Vec::new();
    let mut concat = 0usize;
    for word in ["executables", "executable"] {
        for at in code_words(source, &code, 0, source.len(), word) {
            if at < 2 || b[at - 1] != b'.' || ident_before(source, at - 1).is_empty() {
                continue;
            }
            let op = skip_ws(b, at + word.len());
            let plural = word == "executables";
            let (rhs, appends) =
                if b.get(op) == Some(&b'=') && !matches!(b.get(op + 1), Some(b'=' | b'~' | b'>')) {
                    (skip_ws(b, op + 1), false)
                } else if plural && b[op..].starts_with(b"<<") {
                    (skip_ws(b, op + 2), false)
                } else if plural && b[op..].starts_with(b"+=") {
                    (skip_ws(b, op + 2), true)
                } else if plural && b.get(op) == Some(&b'.') {
                    let (method, after) = read_ident(source, op + 1);
                    match b.get(after) {
                        Some(b'(') if method == "concat" => (skip_ws(b, after + 1), true),
                        Some(b' ' | b'\t') if method == "concat" => (skip_ws(b, after), true),
                        _ => continue,
                    }
                } else {
                    continue;
                };
            let names = ruby_string_list(source, &code, rhs);
            if appends {
                concat += names.len();
            }
            found.extend(names.into_iter().map(|name| (at, name)));
        }
    }
    found.sort_by_key(|(at, _)| *at);
    let bindir = gemspec_bindir(source, &code).unwrap_or_else(|| "bin".to_string());
    let bins = found
        .into_iter()
        .map(|(at, name)| {
            let target = format!("{bindir}/{name}");
            ManifestBin::new(name, Some(target), line_of(source, at))
        })
        .collect();
    (bins, concat)
}

/// GH.20: the literal a gemspec assigns to `<spec>.bindir` (trailing `/`
/// dropped); `None` when it assigns none, or a non-literal.
fn gemspec_bindir(source: &str, code: &CodeMap) -> Option<String> {
    let b = source.as_bytes();
    code_words(source, code, 0, source.len(), "bindir")
        .into_iter()
        .find_map(|at| {
            if at < 2 || b[at - 1] != b'.' || ident_before(source, at - 1).is_empty() {
                return None;
            }
            let op = skip_ws(b, at + "bindir".len());
            if b.get(op) != Some(&b'=') || matches!(b.get(op + 1), Some(b'=' | b'~' | b'>')) {
                return None;
            }
            let lit = quoted_literal(source.get(skip_ws(b, op + 1)..)?)?;
            let dir = lit.trim_end_matches('/');
            (!dir.is_empty()).then(|| dir.to_string())
        })
}

/// GD.21: every executable a `pubspec.yaml` declares, as a flat `cli:<name>`
/// CLI_COMMAND parented to the pubspec's MODULE (as
/// [`extract_manifest_binaries`] mints a manifest's binaries): the keys of
/// the top-level `executables:` map (`dart pub global activate` puts each on
/// the PATH; the value names the `bin/` script, the key is what a caller
/// types). Appended to `nodes` / `nav`; prints the `[cli] php ..` marker
/// when one is read. Any other path reads nothing.
///
/// Stopgap: a pubspec.yaml keeps the engine's yaml route, which calls only
/// `config::extract_yaml_env_defs`, so that function calls this one.
/// Removal: once engine/src/route.rs's yaml branch calls it directly beside
/// `deps_beside`, drop the call from `extract_yaml_env_defs`.
pub(crate) fn extract_pubspec_executables(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
    nodes: &mut Vec<Node>,
    nav: &mut CodeNav,
) {
    if path.rsplit('/').next() != Some("pubspec.yaml") {
        return;
    }
    let mut seen = std::collections::HashSet::new();
    let mut residuals = Residuals::default();
    for name in scan_pubspec_executables(source) {
        if is_bin_name(&name) && seen.insert(name.clone()) {
            add_cli_command(nodes, nav, &name, module_id, repo);
            residuals.pubspec_exec += 1;
        }
    }
    if let Some(line) = residuals.marker(path) {
        eprintln!("{line}");
    }
}

/// The keys of a pubspec's top-level `executables:` map, in order: the
/// indented `name:` / `name: script` lines of the block form (comments and
/// blank lines skipped, the block ending at the next unindented line), or a
/// flow map `executables: {a: , b: c}`. Quoted keys are unquoted; a key
/// holding anything but a plain name is dropped later by [`is_bin_name`].
fn scan_pubspec_executables(source: &str) -> Vec<String> {
    let unquote = |k: &str| k.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
    let strip_comment = |line: &str| -> String {
        match line.find(" #") {
            Some(i) => line[..i].to_string(),
            None if line.trim_start().starts_with('#') => String::new(),
            None => line.to_string(),
        }
    };
    let mut out = Vec::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix("executables:") else {
            continue;
        };
        let rest = strip_comment(rest);
        let rest = rest.trim();
        if let Some(flow) = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
            out.extend(flow.split(',').filter_map(|pair| {
                pair.split_once(':').map(|(k, _)| unquote(k)).or_else(|| {
                    let k = unquote(pair);
                    (!k.is_empty()).then_some(k)
                })
            }));
            break;
        }
        if !rest.is_empty() {
            break;
        }
        for item in lines.by_ref() {
            let text = strip_comment(item);
            if text.trim().is_empty() {
                continue;
            }
            if !text.starts_with([' ', '\t']) {
                break;
            }
            if let Some((key, _)) = text.split_once(':') {
                out.push(unquote(key));
            }
        }
        break;
    }
    out
}

/// The strings a Ruby expression at `at` spells literally: a `[..]` list's
/// plain string elements, a `%w` / `%W` word list, or one string literal
/// ending its statement. Anything else (a method call, a variable) is empty.
fn ruby_string_list(source: &str, code: &CodeMap, at: usize) -> Vec<String> {
    let b = source.as_bytes();
    match b.get(at) {
        Some(b'[') => {
            let Some((start, end)) = balanced_code(source, code, at, MAX_SCRIPTS_SPAN) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            let mut i = start;
            while i < end {
                let next = code.next_code(i);
                if next == i {
                    i += 1;
                    continue;
                }
                let after = skip_ws(b, next);
                if matches!(b[i], b'"' | b'\'')
                    && (after >= end || b[after] == b',')
                    && let Some(lit) = source.get(i..end).and_then(quoted_literal)
                {
                    out.push(lit.to_string());
                }
                i = next;
            }
            out
        }
        Some(b'%') if matches!(b.get(at + 1), Some(b'w' | b'W')) => {
            let Some(&open) = b.get(at + 2) else {
                return Vec::new();
            };
            let close = match open {
                b'[' => b']',
                b'(' => b')',
                b'{' => b'}',
                b'<' => b'>',
                c if c.is_ascii_punctuation() => c,
                _ => return Vec::new(),
            };
            let limit = b.len().min(at + 3 + MAX_SCRIPTS_SPAN);
            let (mut depth, mut j) = (1usize, at + 3);
            while j < limit {
                if b[j] == close {
                    depth -= 1;
                    if depth == 0 {
                        return source[at + 3..j]
                            .split_ascii_whitespace()
                            .map(str::to_string)
                            .collect();
                    }
                } else if b[j] == open && open != close {
                    depth += 1;
                }
                j += 1;
            }
            Vec::new()
        }
        Some(b'"' | b'\'') => {
            let Some(lit) = source.get(at..).and_then(quoted_literal) else {
                return Vec::new();
            };
            let mut j = at + lit.len() + 2;
            while matches!(b.get(j), Some(b' ' | b'\t')) {
                j += 1;
            }
            if matches!(b.get(j), None | Some(b'\n' | b'\r' | b';' | b'#')) {
                vec![lit.to_string()]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// The binary an entry-point spec declares and its target: `name =
/// pkg.mod:func` (spaces optional) -> (`name`, `pkg.mod:func`). `None` when it
/// is not `<bin-name> = <target>`.
fn entry_point(spec: &str) -> Option<(String, String)> {
    let (name, target) = spec.split_once('=')?;
    let (name, target) = (name.trim(), target.trim());
    (is_bin_name(name) && !target.is_empty() && !target.starts_with('='))
        .then(|| (name.to_string(), target.to_string()))
}

/// GH.20: the [`ManifestBin`] of each entry-point line of `text`, which
/// starts at 0-based line `row` of the source.
fn entry_point_lines(text: &str, row: u32) -> impl Iterator<Item = ManifestBin> + '_ {
    (0u32..)
        .zip(text.lines())
        .map_while(|(i, l)| (!l.trim().starts_with('[')).then_some((i, l)))
        .filter_map(move |(i, l)| {
            let (name, target) = entry_point(l.trim())?;
            Some(ManifestBin::new(name, Some(target), row + i))
        })
}

/// LA.20c setup.py: every binary a setuptools `console_scripts` group
/// declares. Three spellings of the group: a `'console_scripts': [...]` dict
/// entry, a `console_scripts=[...]` keyword (`entry_points=dict(...)`), and an
/// `[console_scripts]` section inside an INI-style `entry_points` string. Each
/// `'name = pkg.mod:func'` / `'name=pkg.mod:func'` literal (or INI line)
/// declares `name`. A `'console_scripts'` literal that is not followed by `:`
/// (`entry_points(group='console_scripts')`, `eps['console_scripts']`) is a
/// lookup, not a declaration. GH.20: with each entry's target and line.
fn scan_setup_py_console_scripts(source: &str) -> Vec<ManifestBin> {
    const GROUP: &str = "console_scripts";
    let b = source.as_bytes();
    let code = CodeMap::script(source, Script::Python);
    let mut out = Vec::new();
    for (at, _) in source.match_indices(GROUP) {
        let end = at + GROUP.len();
        let before = at.checked_sub(1).map(|p| b[p]);
        let after = b.get(end).copied();
        let sep = match (before, after) {
            // `'console_scripts': [...]` — the whole literal is the group name.
            (Some(q @ (b'"' | b'\'')), Some(c)) if c == q && code.next_code(at - 1) == end + 1 => {
                let i = skip_ws(b, end + 1);
                (b.get(i) == Some(&b':')).then_some(i)
            }
            // `[console_scripts]` in an INI-style `entry_points` string.
            (Some(b'['), Some(b']')) if !code.is_code(at) => {
                let stop = code.next_code(at).min(end.saturating_add(MAX_SCRIPTS_SPAN));
                let body = source.get(end + 1..stop).unwrap_or("");
                out.extend(entry_point_lines(body, line_of(source, end)));
                continue;
            }
            // `console_scripts=[...]` — a keyword argument.
            _ if code.is_code(at)
                && !before.is_some_and(is_ident_byte)
                && !after.is_some_and(is_ident_byte) =>
            {
                let i = skip_ws(b, end);
                (b.get(i) == Some(&b'=') && b.get(i + 1) != Some(&b'=')).then_some(i)
            }
            _ => None,
        };
        let Some(sep) = sep else { continue };
        let open = skip_ws(b, sep + 1);
        if matches!(b.get(open), Some(b'"' | b'\'')) {
            // One string for the whole group, one entry per line.
            if let Some(lit) = source.get(open..).and_then(quoted_literal) {
                out.extend((0u32..).zip(lit.lines()).filter_map(|(k, l)| {
                    let (name, target) = entry_point(l.trim())?;
                    Some(ManifestBin::new(
                        name,
                        Some(target),
                        line_of(source, open) + k,
                    ))
                }));
            }
            continue;
        }
        if !matches!(b.get(open), Some(b'[' | b'(')) {
            continue;
        }
        let Some((start, stop)) = balanced_code(source, &code, open, MAX_SCRIPTS_SPAN) else {
            continue;
        };
        let mut i = start;
        while i < stop {
            let next = code.next_code(i);
            if next == i {
                i += 1;
                continue;
            }
            if let Some((name, target)) = source
                .get(i..stop)
                .and_then(quoted_literal)
                .and_then(entry_point)
            {
                out.push(ManifestBin::new(name, Some(target), line_of(source, i)));
            }
            i = next;
        }
    }
    out
}

/// FD.5b: true when the needle at `at` is the tail of a path through clap
/// (`clap::Command::new(`, `clap::builder::Command::new(`): the `::`-joined
/// identifier path ending at `at` starts with the `clap` segment.
fn behind_clap_path(source: &str, at: usize) -> bool {
    let b = source.as_bytes();
    let mut s = at;
    while s > 0 && (is_ident_byte(b[s - 1]) || b[s - 1] == b':') {
        s -= 1;
    }
    while s < at && b[s] == b':' {
        s += 1;
    }
    b[s..at].starts_with(b"clap::")
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
    // CL.10 — the JVM's two launch APIs. `ProcessBuilder(` serves Java's
    // `new ProcessBuilder` and Kotlin's bare `ProcessBuilder` alike, each with
    // a quoted argv. A list or array argument has its own needle ending at the
    // inner bracket, so its quoted elements read as the argv; the outer
    // needle's hit at the earlier offset sees an identifier (`List.of`) first
    // and yields nothing. `Runtime.getRuntime().exec` with one string is a
    // shell string, split by `scan_argv`. Not read: `new ProcessBuilder()`
    // with a later `command(..)`, argv lists built elsewhere, and needles split
    // by whitespace (`ProcessBuilder( List.of`).
    // (No quoted literal follows a needle in this comment: the extractor runs
    // over its own source too.)
    "ProcessBuilder(",
    "ProcessBuilder(List.of(",
    "ProcessBuilder(Arrays.asList(",
    "ProcessBuilder(listOf(",
    "Runtime.getRuntime().exec(",
    "Runtime.getRuntime().exec(new String[]{",
    "Runtime.getRuntime().exec(new String[] {",
    "Runtime.getRuntime().exec(arrayOf(",
    // DH.10 — PHP, Dart and Elixir launches. Symfony Process takes one argv
    // array (the opening bracket `scan_argv` steps into), its
    // `fromShellCommandline` and PHP's `shell_exec` / `passthru` a shell
    // string; `proc_open` takes either. dart:io `Process.run` / `runSync` /
    // `start` and Elixir's `System.cmd` take the binary then an argv list,
    // `System.shell` a shell string. C# `new Process()` has no quoted argv
    // and is no site; a bare `exec` needle is not added (`regex.exec(..)`
    // would read a binary): GD.21 reads PHP's global `exec(` in a `.php`
    // file only, outside a member call (`php_residual_sites`).
    "new Process(",
    "Process::fromShellCommandline(",
    "shell_exec(",
    "proc_open(",
    "passthru(",
    "Process.run(",
    "Process.runSync(",
    "Process.start(",
    "System.cmd(",
    "System.shell(",
    // 0.5.5 — Lua's two launches, each a shell string. The paren-less call
    // forms (a string literal straight after the name) are not read.
    "os.execute(",
    "io.popen(",
];

/// `(offset the call's arguments start at, argv)` per launch site.
type ArgvSites = Vec<(usize, Vec<String>)>;

/// GD.21: true for a `.php` path (ASCII case ignored).
fn is_php_path(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("php"))
}

/// The fully qualified Symfony Process class a `new` constructs, the leading
/// `\` optional (a file in the global namespace may omit it).
const SYMFONY_PROCESS_FQ: &str = "Symfony\\Component\\Process\\Process(";

/// GD.21: the PHP launches no [`INVOCATION_PATTERNS`] needle reads, each as
/// `(offset its arguments start at, argv)` in source order, in code only
/// (a PHP-lexed [`CodeMap`]: a comment, string or heredoc launches nothing):
///
/// - PHP's global `exec('convert a.png b.jpg')` (a shell string, split by
///   [`scan_argv`]): `exec` at a word edge, so `shell_exec(` / `curl_exec(`
///   / `pcntl_exec(` are not it, and never a member or static call
///   (`$db->exec(`, `$db?->exec(`, `X::exec(`), a variable or closure
///   (`$exec(`), a concatenation or embedded script (`.exec(`), nor a
///   declaration (`function exec(`). A namespaced `\exec(` is the global
///   function and reads;
/// - `new \Symfony\Component\Process\Process([..])`, the argv array, as the
///   bare `new Process(` needle reads it.
fn php_residual_sites(source: &str) -> (ArgvSites, ArgvSites) {
    let b = source.as_bytes();
    let code = CodeMap::script(source, Script::Php);
    let mut exec = Vec::new();
    let mut from = 0;
    while let Some(rel) = source[from..].find("exec(") {
        let at = from + rel;
        let args = at + "exec(".len();
        from = args;
        let prev = at.checked_sub(1).map(|i| b[i]);
        let member = match prev {
            Some(c) if is_ident_byte(c) || c >= 0x80 => true,
            Some(b'>' | b':' | b'$' | b'.') => true,
            _ => false,
        };
        if member || !code.is_code(at) || php_prev_word(source, at) == "function" {
            continue;
        }
        let tokens = scan_argv(&source[args..]);
        if !tokens.is_empty() {
            exec.push((args, tokens));
        }
    }
    let mut fq = Vec::new();
    let mut from = 0;
    while let Some(rel) = source[from..].find(SYMFONY_PROCESS_FQ) {
        let at = from + rel;
        let args = at + SYMFONY_PROCESS_FQ.len();
        from = args;
        let start = if at > 0 && b[at - 1] == b'\\' {
            at - 1
        } else {
            at
        };
        if !code.is_code(at) || php_prev_word(source, start) != "new" {
            continue;
        }
        let tokens = scan_argv(&source[args..]);
        if !tokens.is_empty() {
            fq.push((args, tokens));
        }
    }
    (exec, fq)
}

/// Most argv tokens [`scan_argv`] reads from one call site (the binary included).
const MAX_ARGV_TOKENS: usize = 8;

/// A13.4 — every CLI_INVOCATION in a file, keyed by the BINARY (`argv[0]`), so
/// it pairs with the `cli:<name>` a cobra/click/clap root declares. The rest of
/// each call's argv is kept on the node's argv cell ([`argv_cell`]), which is
/// how the resolver reaches the subcommand (`mytool migrate` -> `cli:migrate`).
///
/// One node per binary per file: a second `exec.Command("docker", "push")`
/// adds its argv vector to the cell instead of being lost to the dedupe.
///
/// FD.4a: in a Rust or Python file (`path`'s extension) a needle whose first
/// byte sits in a string literal or comment is no call site (CJ.1's
/// `crate::code_guard`), so a scanner's own needle table and test literals
/// mint nothing; `[code-guard] cli-invocation lang=<l> dropped=<n> path=<p>`
/// names a file that refused one. Every other file is scanned as before.
pub fn extract_cli_invocation_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> CliNodes {
    let mut guard = crate::code_guard::LazyGuard::new(path, source);
    // Call-site offsets the guard refused, so two needles ending at one `(`
    // (`os.system(` / `system(`) are one refusal, never two.
    let mut refused: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    // Keyed by the byte offset the arguments start at: `os.system(` and
    // `system(` (or `Command::new(` inside `std::process::Command::new(`) end at
    // the same `(`, so overlapping needles collapse to one call site, and the
    // iteration is in source order. A nested needle (`ProcessBuilder(List.of(`)
    // ends at a later offset than its outer one, whose hit reads no quoted token
    // and so is never a site.
    let mut sites: std::collections::BTreeMap<usize, Vec<String>> =
        std::collections::BTreeMap::new();
    for &pattern in INVOCATION_PATTERNS {
        let mut search_from = 0;
        while let Some(rel_idx) = source[search_from..].find(pattern) {
            let args_at = search_from + rel_idx + pattern.len();
            if refused.contains(&args_at) {
                search_from = args_at;
                continue;
            }
            if let std::collections::btree_map::Entry::Vacant(e) = sites.entry(args_at) {
                let tokens = scan_argv(&source[args_at..]);
                // Only a hit that would mint a site is guarded (and counted):
                // the needle table's own entries read no argv.
                // FD.5b (FD.4a's handoff): `clap::Command::new("x")` is a CLI
                // declaration, not a launch.
                if !tokens.is_empty() && !behind_clap_path(source, search_from + rel_idx) {
                    if guard.admits(search_from + rel_idx) {
                        e.insert(tokens);
                    } else {
                        refused.insert(args_at);
                    }
                }
            }
            search_from = args_at;
        }
    }
    // GD.21: PHP's global `exec(` and a fully qualified Symfony Process,
    // read in a `.php` file only (a bare `exec` needle would read JS
    // `regex.exec(..)` and PDO's `$db->exec(..)`).
    let mut residuals = Residuals::default();
    if is_php_path(path) {
        let (exec, fq) = php_residual_sites(source);
        for (at, tokens) in exec {
            if let std::collections::btree_map::Entry::Vacant(e) = sites.entry(at) {
                e.insert(tokens);
                residuals.php_exec += 1;
            }
        }
        for (at, tokens) in fq {
            if let std::collections::btree_map::Entry::Vacant(e) = sites.entry(at) {
                e.insert(tokens);
                residuals.symfony_fq += 1;
            }
        }
    }
    if let Some(line) = residuals.marker(path) {
        eprintln!("{line}");
    }
    // DG.8: a `scala.sys.process` launch is recognised by what FOLLOWS its
    // argv (`Seq("tool", "x").!`), so no needle above reads it.
    if source.contains("sys.process") {
        let scala = scala_process_sites(source);
        if !scala.is_empty() {
            eprintln!("[cli-inv] scala=sys.process sites={}", scala.len());
        }
        // The same guard: since FD.5b a Scala file is lexed too, so a
        // `Seq(..).!` held in a comment or a plain string is refused. `at` is
        // the first argument byte (a quote, which opens a literal), so the
        // byte before it, inside the call's parentheses, is what is checked.
        for (at, tokens) in scala {
            if guard.admits(at.saturating_sub(1)) {
                sites.entry(at).or_insert(tokens);
            }
        }
    }
    guard.report("cli-invocation");

    // BTreeMap: nodes come out sorted by binary, whatever order the needles hit.
    let mut by_bin: std::collections::BTreeMap<String, Vec<Vec<String>>> =
        std::collections::BTreeMap::new();
    for tokens in sites.into_values() {
        let Some((head, args)) = tokens.split_first() else {
            continue;
        };
        let Some(bin) = binary_name(head) else {
            continue;
        };
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

    CliNodes {
        nodes,
        nav,
        ..CliNodes::default()
    }
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
        let Some(tok) = extract_quoted(rest) else {
            break;
        };
        // Opening quote + literal + closing quote, all ASCII delimiters.
        rest = &rest[tok.len() + 2..];
        tokens.push(tok);
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',' || c == '.');
        if !bracket_open && let Some(r) = strip_open_bracket(rest, &["[]string{", "["]) {
            rest = r.trim_start();
            bracket_open = true;
        }
    }
    if tokens
        .first()
        .is_some_and(|t| t.contains(|c: char| c.is_ascii_whitespace()))
    {
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

/// DG.8: every `scala.sys.process` launch in a file that names `sys.process`
/// in code (an import, or a qualified `scala.sys.process.Process(..)`), as
/// `(offset its argument text starts at, argv)` in source order:
///
/// - a `Seq(..)` / `List(..)` / `Vector(..)` call (an explicit `[String]`
///   type argument stepped over) that is RUN: its `)` followed by `.!`,
///   `.!!`, `.!<`, `.lazyLines`, `.lineStream` (or their `_!` forms),
///   `.run(`, or, on the same line, a postfix ` !` / ` !!` (never `!=`);
/// - `Process(..)` / `Process.apply(..)` whose first argument is such a list
///   call, a string (`Process("git status")`, split as a shell string), or a
///   string then a list (`Process("git", Seq("status"))`): building the
///   ProcessBuilder is the launch, so no run suffix is needed;
/// - a plain string literal that is run (`"git status".!!`), split as a shell
///   string. An interpolated one (`s"git $rev".!!`) is not a literal.
///
/// The argv is read by [`scan_argv`], so it is the quoted tokens only and an
/// argument built elsewhere (`val argv = Seq(..); argv.!`) is no site. A list
/// that is never run (`Seq("a", "b").map(f)`) is no site, and needles in
/// strings and comments are not code ([`CodeMap::new`]: Scala's literal and
/// comment syntax is the C family's).
fn scala_process_sites(source: &str) -> Vec<(usize, Vec<String>)> {
    let b = source.as_bytes();
    let code = CodeMap::new(source);
    let mut from = 0;
    let mut names_sys_process = false;
    while let Some(rel) = source.get(from..).and_then(|s| s.find("sys.process")) {
        let at = from + rel;
        if code.is_code(at) {
            names_sys_process = true;
            break;
        }
        from = at + 1;
    }
    if !names_sys_process {
        return Vec::new();
    }
    let mut out: Vec<(usize, Vec<String>)> = Vec::new();
    for word in ["Seq", "List", "Vector"] {
        for at in code_words(source, &code, 0, source.len(), word) {
            let Some((start, end)) = scala_list_call(source, &code, at, word) else {
                continue;
            };
            if scala_run_suffix(source, end + 1) {
                let tokens = scan_argv(&source[start..end]);
                if !tokens.is_empty() {
                    out.push((start, tokens));
                }
            }
        }
    }
    for at in code_words(source, &code, 0, source.len(), "Process") {
        let mut i = skip_ws(b, at + "Process".len());
        if b.get(i) == Some(&b'.') {
            let (method, after) = read_ident(source, skip_ws(b, i + 1));
            if method != "apply" {
                continue;
            }
            i = after;
        }
        let Some((start, end)) = call_args(source, &code, i) else {
            continue;
        };
        let tokens = scala_process_argv(source, &code, start, end);
        if !tokens.is_empty() {
            out.push((start, tokens));
        }
    }
    for &(start, end) in &code.skips {
        let plain = b[start] == b'"'
            && !b[start..].starts_with(b"\"\"\"")
            && !(start > 0 && is_ident_byte(b[start - 1]));
        if plain && scala_run_suffix(source, end) {
            let tokens = scan_argv(&source[start..end]);
            if !tokens.is_empty() {
                out.push((start, tokens));
            }
        }
    }
    out.sort_by_key(|(at, _)| *at);
    out
}

/// The argument span `(start, end)` of the `Seq` / `List` / `Vector` call
/// whose name `word` starts at `at` (`Seq("a")`, `List[String]("a")`), or
/// `None` when no call follows the name.
fn scala_list_call(source: &str, code: &CodeMap, at: usize, word: &str) -> Option<(usize, usize)> {
    let b = source.as_bytes();
    let mut i = skip_ws(b, at + word.len());
    if b.get(i) == Some(&b'[') {
        let (_, close) = balanced_code(source, code, i, MAX_ATTR_SPAN)?;
        i = close + 1;
    }
    call_args(source, code, i)
}

/// True when the text at `i` (just past a list call's `)` or a string's
/// closing quote) runs a ProcessBuilder: `.!`, `.!!`, `.!<`, `.!!<`, `.run(`,
/// `.lazyLines`, `.lineStream` (and `_!`), or a same-line postfix `!` / `!!`
/// / `!<`. A `!=` comparison is not a run.
fn scala_run_suffix(source: &str, i: usize) -> bool {
    let b = source.as_bytes();
    let bang_run = |j: usize| {
        if b.get(j) != Some(&b'!') {
            return false;
        }
        let mut k = j + 1;
        if b.get(k) == Some(&b'!') {
            k += 1;
        }
        if b.get(k) == Some(&b'<') {
            k += 1;
        }
        b.get(k) != Some(&b'=')
    };
    let j = skip_ws(b, i);
    if b.get(j) == Some(&b'.') {
        let k = j + 1;
        if bang_run(k) {
            return true;
        }
        let (method, after) = read_ident(source, k);
        return match method {
            "lazyLines" | "lineStream" => true,
            "lazyLines_" | "lineStream_" => b.get(after) == Some(&b'!'),
            "run" => b.get(skip_ws(b, after)) == Some(&b'('),
            _ => false,
        };
    }
    let mut p = i;
    while matches!(b.get(p), Some(b' ' | b'\t')) {
        p += 1;
    }
    bang_run(p)
}

/// The argv of `Process(..)`'s argument text `source[start..end]`: a list
/// call's quoted tokens (`Process(Seq("docker", "build"))`), a shell string
/// (`Process("git status")`), or a binary string then an argument list
/// (`Process("git", Seq("status"))`). Anything else (a variable, an
/// interpolation) is no argv.
fn scala_process_argv(source: &str, code: &CodeMap, start: usize, end: usize) -> Vec<String> {
    let b = source.as_bytes();
    let v = skip_ws(b, start);
    if v >= end {
        return Vec::new();
    }
    if b[v] == b'"' {
        let Some(lit) = source.get(v..end).and_then(extract_quoted) else {
            return Vec::new();
        };
        let after = v + lit.len() + 2;
        let mut tokens = scan_argv(&source[v..after]);
        let comma = skip_ws(b, after);
        if !tokens.is_empty() && comma < end && b[comma] == b',' {
            let list = skip_ws(b, comma + 1);
            let (word, _) = read_ident(source, list);
            if matches!(word, "Seq" | "List" | "Vector")
                && let Some((s, e)) = scala_list_call(source, code, list, word)
            {
                tokens.extend(scan_argv(&source[s..e]));
                tokens.truncate(MAX_ARGV_TOKENS);
            }
        }
        return tokens;
    }
    let (word, _) = read_ident(source, v);
    match word {
        "Seq" | "List" | "Vector" => scala_list_call(source, code, v, word)
            .filter(|&(_, e)| e < end)
            .map(|(s, e)| scan_argv(&source[s..e]))
            .unwrap_or_default(),
        _ => Vec::new(),
    }
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
    let payload = ArgvCellPayload {
        bin: std::borrow::Cow::Borrowed(bin),
        argv: argv.to_vec(),
    };
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
            serde_json::from_str::<ArgvCellPayload>(json)
                .map(|p| p.argv)
                .unwrap_or_default()
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
    fn cobra_command_node() {
        let source = r#"var cmd = &cobra.Command{Use: "migrate"}"#;
        let result = extract_cli_command_nodes(source, "go", module_id(), repo());
        assert!(result.nav.qname_by_id.values().any(|q| q == "cli:migrate"));
    }

    #[test]
    fn commander_command_node() {
        let source = "program.command('deploy').description('Deploy app')";
        let result = extract_cli_command_nodes(source, "typescript", module_id(), repo());
        assert!(result.nav.qname_by_id.values().any(|q| q == "cli:deploy"));
    }

    // ---- LA.20a: declarations for compiled languages ----------------------

    fn decls(source: &str, lang: &str) -> CliNodes {
        extract_cli_command_nodes(source, lang, module_id(), repo())
    }

    /// The `cli:<name>` qnames, in emission order.
    fn commands(out: &CliNodes) -> Vec<String> {
        out.nodes
            .iter()
            .filter_map(|n| out.nav.qname_by_id.get(&n.id).cloned())
            .collect()
    }

    /// Every HANDLED_BY ref as (command qname, handler).
    fn handlers(out: &CliNodes) -> Vec<(String, CallQualifier)> {
        out.refs
            .iter()
            .map(|r| {
                assert_eq!(r.category, edge_category::HANDLED_BY);
                assert_eq!(r.from_module, module_id());
                (
                    out.nav
                        .qname_by_id
                        .get(&r.from)
                        .cloned()
                        .unwrap_or_default(),
                    r.qualifier.clone(),
                )
            })
            .collect()
    }

    fn bare(name: &str) -> CallQualifier {
        CallQualifier::Bare(name.to_string())
    }

    /// The matrix/rust/cli_def probe, read from the fixture so this file never
    /// carries a `#[command(name` line of its own for glia's self-scan to read.
    const CLAP_PROBE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/rust/cli_def/server/src/main.rs"
    ));

    #[test]
    fn clap_multi_line_attr_and_subcommand_variants() {
        let out = decls(CLAP_PROBE, "rust");
        // The one-line name attribute on `GarbageCollect` still comes from the
        // line reader first; the span scan adds the multi-line root and the
        // kebab-cased variant.
        assert_eq!(
            commands(&out),
            vec!["cli:gc", "cli:shopctl", "cli:sync-users"]
        );
        assert_eq!(out.per_framework, vec![(CliFramework::Clap, 3)]);
        assert!(out.refs.is_empty());
        assert_eq!(
            decl_marker("rust", &out, "src/main.rs").as_deref(),
            Some("[cli-decl] rust=clap:3 handler_refs=0 path=src/main.rs")
        );
    }

    #[test]
    fn clap_subcommand_field_attr_names_nothing() {
        let source = "use clap::Parser;\n#[derive(Parser)]\nstruct Cli {\n    #[command(subcommand)]\n    command: Commands,\n    #[arg(long, name = \"force\")]\n    force: bool,\n}\n";
        let out = decls(source, "rust");
        assert!(commands(&out).is_empty(), "{:?}", commands(&out));
        assert_eq!(decl_marker("rust", &out, "src/main.rs"), None);
    }

    #[test]
    fn clap_rename_all_override_and_unnamed_variants() {
        let source = r#"use clap::Subcommand;
#[derive(Debug, clap::Subcommand)]
#[command(rename_all = "snake_case", about = "name = \"not-this\"")]
pub(crate) enum Ops {
    /// Doc, with a comma and an (unbalanced paren
    SyncUsers,
    #[clap(name = "gc")]
    GarbageCollect { #[arg(long)] dry: bool },
    Import(ImportArgs),
    #[command(flatten)]
    Admin(AdminOps),
    #[command(external_subcommand)]
    External(Vec<String>),
}
"#;
        let out = decls(source, "rust");
        assert_eq!(
            commands(&out),
            vec!["cli:sync_users", "cli:gc", "cli:import"]
        );
    }

    #[test]
    fn clap_parser_enum_variants_are_commands() {
        let source = "use clap::Parser;\n#[derive(Parser)]\nenum Cli { Build, RunAll(RunArgs) }\n";
        assert_eq!(
            commands(&decls(source, "rust")),
            vec!["cli:build", "cli:run-all"]
        );
    }

    #[test]
    fn clap_kebab_case_matches_heck() {
        for (variant, want) in [
            ("SyncUsers", "sync-users"),
            ("HTTPServe", "http-serve"),
            ("V2Api", "v2-api"),
            ("Sha256Sum", "sha256-sum"),
            ("Gc", "gc"),
            ("ABC", "abc"),
            ("Snake_Case", "snake-case"),
        ] {
            assert_eq!(clap_rename(variant, None), want, "{variant}");
        }
        assert_eq!(clap_rename("SyncUsers", Some("kebab-case")), "sync-users");
        assert_eq!(clap_rename("SyncUsers", Some("snake_case")), "sync_users");
        assert_eq!(clap_rename("SyncUsers", Some("lower")), "syncusers");
        assert_eq!(clap_rename("SyncUsers", Some("verbatim")), "SyncUsers");
        assert_eq!(clap_rename("SyncUsers", Some("camelCase")), "syncUsers");
        assert_eq!(
            clap_rename("SyncUsers", Some("SCREAMING_SNAKE_CASE")),
            "SYNC_USERS"
        );
        assert_eq!(clap_rename("SyncUsers", Some("no-such-rule")), "sync-users");
    }

    #[test]
    fn clap_builder_names_but_std_process_command_does_not() {
        let builder = "use clap::{Arg, Command};\nfn cli() -> Command {\n    Command::new(\"shopctl\").subcommand(Command::new(\"migrate\"))\n}\n";
        assert_eq!(
            commands(&decls(builder, "rust")),
            vec!["cli:shopctl", "cli:migrate"]
        );

        // std's Command is an invocation, even with a clap derive beside it.
        let std_cmd = "use clap::Subcommand;\nuse std::process::{Command, Stdio};\nfn f() { Command::new(\"git\").spawn(); }\n";
        assert!(commands(&decls(std_cmd, "rust")).is_empty());
        // Both in scope: only the fully qualified clap form counts.
        let both = "use clap::Command;\nuse std::process::Command as _;\nuse tokio::process::Command;\nfn f() { Command::new(\"git\"); clap::Command::new(\"tool\"); }\n";
        assert_eq!(commands(&decls(both, "rust")), vec!["cli:tool"]);
        // clap 2 / 3 builders.
        let old = "use clap::{App, SubCommand};\nfn f() { App::new(\"old\").subcommand(SubCommand::with_name(\"sub\")); }\n";
        assert_eq!(commands(&decls(old, "rust")), vec!["cli:old", "cli:sub"]);
    }

    #[test]
    fn clap_scans_are_gated_on_clap() {
        let source = "#[derive(Subcommand)]\nenum Commands { SyncUsers }\n";
        assert!(commands(&decls(source, "rust")).is_empty());
    }

    #[test]
    fn picocli_class_and_method_commands() {
        let source = r#"package com.example;

import picocli.CommandLine;
import picocli.CommandLine.Command;

@Command(name = "invctl", mixinStandardHelpOptions = true, subcommands = {Export.class})
public class Tool implements Runnable {
    @Command(name = "purge", description = "Purge (all) \"records\"")
    public int purge(@Option(names = "-f") boolean force) { return 0; }

    @CommandLine.Command(name = "list")
    List<String> listAll() { return null; }

    public void run() {}
}

@Command(name = "export", description = "Export inventory")
@SuppressWarnings("unused")
final class Export implements Runnable {
    public void run() {}
}
"#;
        let out = decls(source, "java");
        assert_eq!(
            commands(&out),
            vec!["cli:invctl", "cli:purge", "cli:list", "cli:export"]
        );
        let attr = |base: &str, name: &str| CallQualifier::Attribute {
            base: base.to_string(),
            name: name.to_string(),
        };
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:invctl".to_string(), bare("Tool")),
                ("cli:purge".to_string(), attr("Tool", "purge")),
                ("cli:list".to_string(), attr("Tool", "listAll")),
                ("cli:export".to_string(), bare("Export")),
            ]
        );
        assert_eq!(
            decl_marker("java", &out, "Tool.java").as_deref(),
            Some("[cli-decl] java=picocli:4 handler_refs=4 path=Tool.java")
        );
        // Kotlin is read under its own tag (A14.2 moved `.kt` off java), and
        // still under the java tag extract.rs's A14.2 stopgap passes.
        let kt = "import picocli.CommandLine.Command\n@Command(name = \"kt\")\nclass KtTool : Runnable { override fun run() {} }\n";
        assert_eq!(
            handlers(&decls(kt, "kotlin")),
            vec![("cli:kt".to_string(), bare("KtTool"))]
        );
        assert_eq!(
            handlers(&decls(kt, "java")),
            vec![("cli:kt".to_string(), bare("KtTool"))]
        );
        // No picocli in the file: no scan.
        assert!(commands(&decls("@Command(name = \"x\") class X {}", "java")).is_empty());
    }

    #[test]
    fn system_commandline_name_not_root_description() {
        let source = r#"using System.CommandLine;

var root = new RootCommand("billing tool");
var sync = new Command("reconcile", "Reconcile invoices");
sync.SetHandler(Reconcile);
Command other = new Command(name: "export");
other.SetHandler(ctx => Run(ctx));
var third = new Command("audit");
third.SetAction(Handlers.Audit);
root.AddCommand(sync);
"#;
        let out = decls(source, "csharp");
        assert_eq!(
            commands(&out),
            vec!["cli:reconcile", "cli:export", "cli:audit"]
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:reconcile".to_string(), bare("Reconcile")),
                (
                    "cli:audit".to_string(),
                    CallQualifier::Attribute {
                        base: "Handlers".into(),
                        name: "Audit".into()
                    }
                ),
            ]
        );
        assert_eq!(
            out.per_framework,
            vec![(CliFramework::SystemCommandLine, 3)]
        );
    }

    #[test]
    fn spectre_add_command_and_branch() {
        let source = r#"using Spectre.Console.Cli;

namespace Billing;

public static class SpectreHost
{
    public static int Run(string[] args)
    {
        var app = new CommandApp();
        app.Configure(c => {
            c.AddCommand<SyncCommand>("sync");
            c.AddBranch<AdminSettings>("admin", a => a.AddCommand<Commands.PurgeCommand>("purge"));
        });
        return app.Run(args);
    }
}
"#;
        let out = decls(source, "csharp");
        assert_eq!(commands(&out), vec!["cli:sync", "cli:purge", "cli:admin"]);
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:sync".to_string(), bare("SyncCommand")),
                ("cli:purge".to_string(), bare("PurgeCommand")),
            ]
        );
        assert_eq!(
            decl_marker("csharp", &out, "SpectreHost.cs").as_deref(),
            Some("[cli-decl] csharp=spectre:3 handler_refs=2 path=SpectreHost.cs")
        );
        // Without the Spectre import, a generic AddCommand is not read.
        assert!(commands(&decls("c.AddCommand<SyncCommand>(\"sync\");", "csharp")).is_empty());
    }

    #[test]
    fn language_gating() {
        // Python keeps its click and Typer explicit decorator names.
        let typer =
            "import typer\napp = typer.Typer()\n@app.command(\"purge\")\ndef purge(): ...\n";
        let out = decls(typer, "python");
        assert_eq!(commands(&out), vec!["cli:purge"]);
        assert_eq!(out.per_framework, vec![(CliFramework::Typer, 1)]);
        let click = "import click\n@click.command(\"sync\")\ndef sync(): ...\n@cli.command(\"gc\")\ndef gc(): ...\n";
        let out = decls(click, "python");
        assert_eq!(commands(&out), vec!["cli:sync", "cli:gc"]);
        assert_eq!(out.per_framework, vec![(CliFramework::Click, 2)]);
        // Commander's needle is TS-family only (LA.20b: Python too - pymongo's
        // `db.command("ping")` is not a declaration).
        for lang in ["ruby", "java", "go", "php", "rust", "csharp", "python"] {
            assert!(
                commands(&decls("runner.command(\"y\")", lang)).is_empty(),
                "{lang}"
            );
        }
        for lang in ["typescript", "react", "angular", "vue"] {
            assert_eq!(
                commands(&decls("program.command('y')", lang)),
                vec!["cli:y"],
                "{lang}"
            );
        }
        // Cobra is Go-only; clap's line form is Rust-only.
        assert!(commands(&decls("x := &cobra.Command{Use: \"m\"}", "typescript")).is_empty());
        assert!(commands(&decls("#[command(name = \"n\")]", "python")).is_empty());
        assert!(commands(&decls("anything", "swift")).is_empty());
    }

    #[test]
    fn attr_span_is_bounded_quote_aware_and_char_safe() {
        let s = "#[command(name = \"é\", about = \"naïve ) (\", c = ')')] x";
        assert_eq!(
            attr_span(s, 9),
            Some("name = \"é\", about = \"naïve ) (\", c = ')'")
        );
        assert_eq!(attr_span(s, 0), None, "not an opening bracket");
        assert_eq!(attr_span(s, s.len() + 10), None, "out of range");
        assert_eq!(attr_span("(\"unterminated)", 0), None);
        assert_eq!(attr_span("(a]", 0), None, "mismatched closer");
        assert_eq!(attr_span("(a // )\n)", 0), Some("a // )\n"));
        let long = format!("({})", "é".repeat(MAX_ATTR_SPAN));
        assert_eq!(attr_span(&long, 0), None, "over the cap");
        assert_eq!(
            attr_kv("about = \"name = 'x'\", name = \"real\"", "name").as_deref(),
            Some("real")
        );
        assert_eq!(attr_kv("bin_name = \"no\"", "name"), None);
    }

    #[test]
    fn code_map_skips_literals_and_comments() {
        let source = "a \"b\\\"\" r#\"c\"# 'd' '\\'' /* e /* f */ g */ h // i\nj @\"k\"\"\" l \"\"\"m\"\"\" n <'o> p";
        let code = CodeMap::new(source);
        let at = |c: char| source.find(c).map(|i| code.is_code(i));
        for c in ['a', 'h', 'j', 'l', 'n', 'o', 'p'] {
            assert_eq!(at(c), Some(true), "{c} is code");
        }
        for c in ['b', 'c', 'd', 'e', 'f', 'g', 'i', 'k', 'm'] {
            assert_eq!(at(c), Some(false), "{c} is inside a literal or comment");
        }
    }

    #[test]
    fn needles_in_strings_and_comments_declare_nothing() {
        let rust = r##"use clap::{Command, Subcommand};
/// Example: `Command::new("doc-example")`
/// #[derive(Subcommand)] enum Doc { DocVariant }
const NEEDLES: &[(&str, bool)] = &[("Command::new(", true), ("x", false)];
const RAW: &str = r#"#[derive(Subcommand)] enum Raw { RawVariant } Command::new("raw")"#;
fn real() { Command::new("real"); }
"##;
        assert_eq!(commands(&decls(rust, "rust")), vec!["cli:real"]);
        let java = "import picocli.CommandLine.Command;\n/** e.g. @Command(name = \"javadoc\") */\n// @Command(name = \"line\")\nString s = \"@Command(name = \\\"quoted\\\")\";\n@Command(name = \"real\") class Real {}\n";
        assert_eq!(commands(&decls(java, "java")), vec!["cli:real"]);
        let csharp = "using System.CommandLine;\nusing Spectre.Console.Cli;\n// var x = new Command(\"commented\");\nvar s = @\"new Command(\"\"verbatim\"\")\";\n/* c.AddCommand<X>(\"block\"); */\nvar r = new Command(\"real\");\n";
        assert_eq!(commands(&decls(csharp, "csharp")), vec!["cli:real"]);
    }

    #[test]
    fn truncated_and_multibyte_sources_never_panic() {
        let sources = [
            (CLAP_PROBE, "rust"),
            (
                "use clap::Command; fn f() { Command::new(\"ä\"); } #[derive(Subcommand)] enum E { Ä, B }",
                "rust",
            ),
            (
                "import picocli.X; @Command(name = \"é\") class Ü { @Command(name=\"m\") void ä() {} }",
                "java",
            ),
            (
                "using System.CommandLine; var ü = new Command(\"é\"); ü.SetHandler(Ä); using Spectre.Console.Cli; c.AddCommand<Ö>(\"x\"); c.AddBranch<",
                "csharp",
            ),
        ];
        for (source, lang) in sources {
            for (cut, _) in source.char_indices() {
                let _ = decls(&source[..cut], lang);
            }
            let _ = decls(source, lang);
        }
    }

    // ---- LA.20b: declarations for scripting languages ----------------------

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute {
            base: base.to_string(),
            name: name.to_string(),
        }
    }

    const THOR_PROBE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/ruby/cli_def/server/lib/deployer.rb"
    ));
    const SYMFONY_PROBE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/php/cli_def/server/src/Command/SyncCommand.php"
    ));
    const LARAVEL_PROBE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/php/cli_def/server/app/Console/Commands/SendEmails.php"
    ));
    const TYPER_ARGPARSE_PROBE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/fixtures/py-typer-argparse-cli/tcli.py"
    ));
    const CLICK_PROBE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/python/cli_def/server/definer.py"
    ));

    #[test]
    fn thor_desc_names_the_next_method() {
        let out = decls(THOR_PROBE, "ruby");
        assert_eq!(commands(&out), vec!["cli:rollout"]);
        assert_eq!(
            handlers(&out),
            vec![("cli:rollout".to_string(), attr("Deployer", "rollout"))]
        );
        assert_eq!(
            decl_marker("ruby", &out, "lib/deployer.rb").as_deref(),
            Some("[cli-decl] ruby=thor:1 handler_refs=1 path=lib/deployer.rb")
        );
    }

    #[test]
    fn thor_private_no_commands_and_undescribed_methods_are_not_commands() {
        let source = r#"require "thor"

module Ops
  class Cli < ::Thor
    # desc "commented ENV", "not a command"
    desc "rollout-all ENV", "roll out everywhere"
    method_option :force, type: :boolean, desc: "skip checks"
    long_desc <<~LONGDESC
desc "heredoc", "not a command"
def heredoc
end
    LONGDESC
    def rollout_all(env)
      helper
    end

    desc("status", "show status")
    def status; end

    desc "fetch URL", "usage word differs from the method"
    def download(url)
    end

    def undescribed
    end

    desc "self_x", "a singleton method takes no desc"
    def self.self_x; end
    def next_public
    end

    subcommand "db", Ops::DbCli

    no_commands do
      desc "hidden", "inside no_commands"
      def hidden; end
    end

    desc "later", "a private method does not take a desc"
    private

    def later; end
  end
end

class Steps < Thor::Group
  desc "step", "a group step"
  def step; end
end
"#;
        let out = decls(source, "ruby");
        assert_eq!(
            commands(&out),
            vec![
                "cli:rollout-all",
                "cli:status",
                "cli:download",
                "cli:next_public",
                "cli:db"
            ]
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:rollout-all".to_string(), attr("Cli", "rollout_all")),
                ("cli:status".to_string(), attr("Cli", "status")),
                ("cli:download".to_string(), attr("Cli", "download")),
                ("cli:next_public".to_string(), attr("Cli", "next_public")),
                ("cli:db".to_string(), bare("DbCli")),
            ]
        );
        // No `< Thor` class: nothing, even with desc + def.
        let plain = "class Thorough < Base\n  desc \"x\", \"y\"\n  def x; end\nend\n";
        assert!(commands(&decls(plain, "ruby")).is_empty());
    }

    #[test]
    fn symfony_attribute_and_default_name() {
        let out = decls(SYMFONY_PROBE, "php");
        assert_eq!(
            commands(&out),
            vec!["cli:app:sync-orders", "cli:app:legacy-import"]
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:app:sync-orders".to_string(), bare("SyncCommand")),
                ("cli:app:legacy-import".to_string(), bare("LegacyCommand")),
            ]
        );
        assert_eq!(
            decl_marker("php", &out, "src/Command/SyncCommand.php").as_deref(),
            Some("[cli-decl] php=symfony:2 handler_refs=2 path=src/Command/SyncCommand.php")
        );
        let more = r#"<?php
use Symfony\Component\Console\Attribute\AsCommand;
use Symfony\Component\Console\Command\Command;

// #[AsCommand(name: 'commented')]
#[AsCommand('app:create-user|app:add-user', 'Creates a user (and more')]
#[\Some\Other]
final class CreateUser extends Command {}

class Configured extends Command
{
    protected function configure(): void
    {
        $this->setName('app:configured')->setDescription('x');
        $user->setName('not-a-command');
        $name = 'local-variable';
    }
}

class Plain
{
    protected static $defaultName = 'not-console';
    protected $signature = 'sha256=abc';
}
"#;
        let out = decls(more, "php");
        assert_eq!(
            commands(&out),
            vec![
                "cli:app:create-user",
                "cli:app:add-user",
                "cli:app:configured"
            ]
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:app:create-user".to_string(), bare("CreateUser")),
                ("cli:app:add-user".to_string(), bare("CreateUser")),
                ("cli:app:configured".to_string(), bare("Configured")),
            ]
        );
    }

    #[test]
    fn laravel_signature_with_args_and_options() {
        let out = decls(LARAVEL_PROBE, "php");
        assert_eq!(commands(&out), vec!["cli:emails:send"]);
        assert_eq!(
            handlers(&out),
            vec![("cli:emails:send".to_string(), bare("SendEmails"))]
        );
        assert_eq!(out.per_framework, vec![(CliFramework::Laravel, 1)]);
        let multi = "<?php\nuse Illuminate\\Console\\Command;\nclass Prune extends \\Illuminate\\Console\\Command\n{\n    protected $signature = \"model:prune\n        {--model=* : Class names}\n        {--pretend}\";\n}\nclass Legacy extends Command\n{\n    protected $name = 'legacy:run';\n    public function handle() { $signature = 'local {x}'; }\n}\n";
        let out = decls(multi, "php");
        assert_eq!(commands(&out), vec!["cli:model:prune", "cli:legacy:run"]);
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:model:prune".to_string(), bare("Prune")),
                ("cli:legacy:run".to_string(), bare("Legacy")),
            ]
        );
    }

    #[test]
    fn typer_bare_explicit_and_add_typer() {
        let out = decls(TYPER_ARGPARSE_PROBE, "python");
        assert_eq!(
            commands(&out),
            vec!["cli:import-users", "cli:purge", "cli:dbtool", "cli:migrate"]
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:import-users".to_string(), bare("import_users")),
                ("cli:purge".to_string(), bare("purge_all")),
                ("cli:migrate".to_string(), bare("run_migrate")),
            ]
        );
        assert_eq!(
            decl_marker("python", &out, "tcli.py").as_deref(),
            Some("[cli-decl] python=typer:2,argparse:2 handler_refs=3 path=tcli.py")
        );
        let source = r#"from typer import Typer
import typer

app = Typer()
users = typer.Typer()
app.add_typer(users, name="users")
app.add_typer(other)


@users.command(name="list")
@some.decorator(
    "multi-line )",
)
async def list_users():
    """@app.command() in a docstring"""


# @app.command()
@app.command(help="no name here")
def Sync_Now():
    pass


@app.callback()
def main():
    pass


app.command(name="serve")(start_server)
app.command()(tasks.run_all)
"#;
        let out = decls(source, "python");
        assert_eq!(
            commands(&out),
            vec![
                "cli:users",
                "cli:list",
                "cli:sync-now",
                "cli:serve",
                "cli:run-all"
            ]
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:list".to_string(), bare("list_users")),
                ("cli:sync-now".to_string(), bare("Sync_Now")),
                ("cli:serve".to_string(), bare("start_server")),
                ("cli:run-all".to_string(), attr("tasks", "run_all")),
            ]
        );
        assert_eq!(out.per_framework, vec![(CliFramework::Typer, 5)]);
    }

    #[test]
    fn click_bare_commands_and_groups() {
        // The committed python/cli_def probe: the bare group is not minted, and
        // `sync`'s handler would bind to `cli:sync` itself (see the next test).
        let out = decls(CLICK_PROBE, "python");
        assert_eq!(commands(&out), vec!["cli:sync"]);
        assert!(handlers(&out).is_empty());
        let source = "import click\n\n@click.group()\ndef cli(): ...\n\n@cli.group()\ndef db(): ...\n\n@db.command()\ndef init_db_command(): ...\n\n@click.command\ndef export_users(): ...\n\n@cli.command('seed')\ndef seed_cmd(): ...\n";
        let out = decls(source, "python");
        assert_eq!(
            commands(&out),
            vec!["cli:init-db", "cli:export-users", "cli:seed"]
        );
        assert_eq!(out.per_framework, vec![(CliFramework::Click, 3)]);
        // No typer / click import: explicit decorator names only.
        let ungated = "from .main import cli\n\n@cli.command(\"sync\")\ndef sync(): ...\n\n@bot.command()\ndef ping(): ...\n";
        assert_eq!(commands(&decls(ungated, "python")), vec!["cli:sync"]);
    }

    /// The LA.20b stopgap for `module_symbols` shadowing: a new-form name
    /// equal to a top-level def is not minted, and a `Bare` handler named like
    /// a command of the same file is not emitted.
    #[test]
    fn names_that_would_shadow_a_module_symbol() {
        let source = r#"import argparse
import click


@click.command()
def main():
    pass


@click.command("sync")
def sync():
    pass


@click.command(name="status")
def status():
    pass


def migrate(args):
    pass


def build():
    p = argparse.ArgumentParser(prog="build")
    sub = p.add_subparsers()
    m = sub.add_parser("migrate")
    m.set_defaults(func=migrate)
    s = sub.add_parser("seed")
    s.set_defaults(func=migrate)
    m = sub.add_parser(NAME)
    m.set_defaults(func=other)
"#;
        let out = decls(source, "python");
        assert_eq!(commands(&out), vec!["cli:sync", "cli:seed"]);
        assert_eq!(
            handlers(&out),
            vec![("cli:seed".to_string(), bare("migrate"))]
        );
    }

    #[test]
    fn argparse_subparsers_and_set_defaults() {
        let source = r#"import argparse
from app import commands


def build():
    parser = argparse.ArgumentParser(description="no prog")
    sub = parser.add_subparsers(dest="cmd")
    p = sub.add_parser("init", help="init things")
    p.set_defaults(func=commands.do_init)
    p = sub.add_parser("serve")
    p.set_defaults(verbose=False, handler=start_server)
    sub.add_parser("check").set_defaults(func=run_check)
    q = sub.add_parser("lazy")
    q.set_defaults(func=lambda a: a)
    # sub.add_parser("commented")
    return parser
"#;
        let out = decls(source, "python");
        assert_eq!(
            commands(&out),
            vec!["cli:init", "cli:serve", "cli:check", "cli:lazy"]
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:init".to_string(), attr("commands", "do_init")),
                ("cli:serve".to_string(), bare("start_server")),
                ("cli:check".to_string(), bare("run_check")),
            ]
        );
        assert_eq!(out.per_framework, vec![(CliFramework::Argparse, 4)]);
        // No argparse import: `add_parser` is someone else's API.
        assert!(commands(&decls("p = registry.add_parser(\"x\")\n", "python")).is_empty());
    }

    #[test]
    fn script_code_map_skips_literals_comments_and_heredocs() {
        let at = |src: &str, code: &CodeMap, c: char| src.find(c).map(|i| code.is_code(i));
        let py = "a # b\n'c' \"d\" '''e\n''' f \"\"\"g\"\"\" h";
        let code = CodeMap::script(py, Script::Python);
        for c in ['a', 'f', 'h'] {
            assert_eq!(at(py, &code, c), Some(true), "{c} is code");
        }
        for c in ['b', 'c', 'd', 'e', 'g'] {
            assert_eq!(at(py, &code, c), Some(false), "{c} is not code");
        }
        let rb = "a <<~X, b\nc\n  X\nd # e\n=begin\nf\n=end\nk 'h' arr << I";
        let code = CodeMap::script(rb, Script::Ruby);
        for c in ['a', 'b', 'd', 'k', 'I'] {
            assert_eq!(at(rb, &code, c), Some(true), "{c} is code");
        }
        for c in ['c', 'e', 'f', 'h'] {
            assert_eq!(at(rb, &code, c), Some(false), "{c} is not code");
        }
        let php = "#[A] b // c\n# d\n/* e */ f 'g' <<<'X'\nh\nX;\ni";
        let code = CodeMap::script(php, Script::Php);
        for c in ['A', 'b', 'f', 'i'] {
            assert_eq!(at(php, &code, c), Some(true), "{c} is code");
        }
        for c in ['c', 'd', 'e', 'g', 'h'] {
            assert_eq!(at(php, &code, c), Some(false), "{c} is not code");
        }
    }

    #[test]
    fn scripting_scans_never_panic_on_truncated_or_multibyte_sources() {
        let sources = [
            (THOR_PROBE, "ruby"),
            (SYMFONY_PROBE, "php"),
            (LARAVEL_PROBE, "php"),
            (TYPER_ARGPARSE_PROBE, "python"),
            (
                "class Ü < Thor\n  desc \"é ENV\", \"ü\"\n  def é; end\n  subcommand \"ö\", Ä\nend\n",
                "ruby",
            ),
            (
                "<?php #[AsCommand(name: 'é|ü')] class Ä extends Command { protected $signature = 'ö {x}'; } <<<É\n",
                "php",
            ),
            (
                "import typer, argparse\n@app.command()\ndef é(): ...\nm = s.add_parser(\"ü\")\nm.set_defaults(func=ö)\n",
                "python",
            ),
        ];
        for (source, lang) in sources {
            for (cut, _) in source.char_indices() {
                let _ = decls(&source[..cut], lang);
            }
            let _ = decls(source, lang);
        }
    }

    #[test]
    fn detects_exec_command_invocation() {
        let source = "cmd := exec.Command(\"docker\", \"build\", \".\")";
        let result = extract_cli_invocation_nodes(source, "", module_id(), repo());
        assert!(
            result
                .nav
                .qname_by_id
                .values()
                .any(|q| q == "cli_invoke:docker")
        );
    }

    /// The single qname and the argv vectors of its cell(s).
    fn invocation(source: &str) -> (Vec<String>, Vec<Vec<String>>) {
        let result = extract_cli_invocation_nodes(source, "", module_id(), repo());
        let qnames = result
            .nodes
            .iter()
            .filter_map(|n| result.nav.qname_by_id.get(&n.id).cloned())
            .collect();
        let argv = result
            .nodes
            .iter()
            .flat_map(|n| n.cells.iter().flat_map(parse_argv_cell))
            .collect();
        (qnames, argv)
    }

    /// The sorted qnames a file at `path` mints.
    fn invocation_qnames(source: &str, path: &str) -> Vec<String> {
        let result = extract_cli_invocation_nodes(source, path, module_id(), repo());
        let mut q: Vec<String> = result
            .nodes
            .iter()
            .filter_map(|n| result.nav.qname_by_id.get(&n.id).cloned())
            .collect();
        q.sort();
        q
    }

    #[test]
    fn clap_command_paths_are_declarations_not_launches() {
        // FD.5b (FD.4a's handoff): clap's builder is no launch.
        let src = "fn p() { let c = clap::Command::new(\"tests\").no_binary_name(true); \
                   let d = ::clap::builder::Command::new(\"x\"); \
                   std::process::Command::new(\"git\").spawn(); }\n";
        assert_eq!(invocation_qnames(src, "src/a.rs"), ["cli_invoke:git"]);
        assert!(behind_clap_path("clap::Command::new(", 6));
        assert!(!behind_clap_path("std::process::Command::new(", 15));
        assert!(!behind_clap_path("myclap::Command::new(", 8));
    }

    #[test]
    fn invocations_in_compiled_literals_and_comments_mint_nothing() {
        // FD.5b: Go, JS / TS and the JVM languages are guarded too.
        for (path, src) in [
            (
                "cmd/a.go",
                "// exec.Command(\"ghost\")\nconst d = `exec.Command(\"ghost\")`\n",
            ),
            (
                "src/a.ts",
                "const t = `execSync('ghost')`; // spawnSync('ghost')\n",
            ),
            (
                "src/A.kt",
                "/* ProcessBuilder(\"ghost\") */ val s = \"ProcessBuilder(\\\"ghost\\\")\"\n",
            ),
            ("src/A.scala", "// Seq(\"ghost\", \"x\").!\n"),
        ] {
            assert!(invocation_qnames(src, path).is_empty(), "{path}: {src:?}");
        }
        assert_eq!(
            invocation_qnames("cmd := exec.Command(\"git\", \"push\")\n", "cmd/a.go"),
            ["cli_invoke:git"]
        );
        assert_eq!(
            invocation_qnames("const t = `${execSync('git status')}`;\n", "src/a.ts"),
            ["cli_invoke:git"]
        );
        // A Scala launch in code is read through the guard (DG.8's sites).
        let scala =
            "import scala.sys.process._\nobject E { def r() = Seq(\"invctl\", \"export\").! }\n";
        assert_eq!(
            invocation_qnames(scala, "src/E.scala"),
            ["cli_invoke:invctl"]
        );
    }

    #[test]
    fn invocations_in_rust_literals_and_comments_mint_nothing() {
        // FD.4a: a needle inside a string literal or a comment is no launch.
        for src in [
            "let t = r#\"std::process::Command::new(\"x\")\"#;\n",
            "let t = r#\"fn real() { Command::new(\"x\"); }\"#;\n",
            "// Command::new(\"x\")\nfn f() {}\n",
            "/// subprocess.run(['x', 'y'])\nfn f() {}\n",
        ] {
            assert!(invocation_qnames(src, "src/a.rs").is_empty(), "{src:?}");
        }
        // In code it is read, also after a refused one.
        let code = "// Command::new(\"ghost\")\nfn f() { Command::new(\"x\").spawn(); }\n";
        assert_eq!(invocation_qnames(code, "src/a.rs"), ["cli_invoke:x"]);
        // The same literal in a file with no guard is read as before.
        let lit = "let t = r#\"fn real() { Command::new(\"x\"); }\"#;\n";
        assert_eq!(invocation_qnames(lit, "src/a.vue"), ["cli_invoke:x"]);
    }

    #[test]
    fn invocations_in_python_literals_and_comments_mint_nothing() {
        for src in [
            "S = \"subprocess.run(['x', 'y'])\"\n",
            "# os.system('x')\n",
            "'''\nsubprocess.Popen(['x'])\n'''\n",
        ] {
            assert!(invocation_qnames(src, "tools/a.py").is_empty(), "{src:?}");
        }
        assert_eq!(
            invocation_qnames("subprocess.run(['x', 'y'])\n", "tools/a.py"),
            ["cli_invoke:x"]
        );
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
        let result = extract_cli_invocation_nodes(source, "", module_id(), repo());
        assert_eq!(result.nodes.len(), 1);
        assert_eq!(result.nodes[0].cells.len(), 1);
        let CellPayload::Json(json) = &result.nodes[0].cells[0].payload else {
            panic!("argv cell is JSON")
        };
        assert_eq!(
            json,
            r#"{"bin":"docker","argv":[["build","."],["push","x"]]}"#
        );
    }

    #[test]
    fn path_qualified_binary_keys_on_basename() {
        assert_eq!(
            invocation("subprocess.run(['/usr/bin/kubectl', 'get'])").0,
            vec!["cli_invoke:kubectl"]
        );
        assert_eq!(
            invocation("Process.Start(\"C:\\\\tools\\\\deploy.exe\")").0,
            vec!["cli_invoke:deploy"]
        );
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

    // ---- CL.10: JVM process launches ----------------------------------------

    /// The matrix/java/cli_inv probe's launcher, read from the fixture.
    const JAVA_CLI_INV_EXPORTER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/java/cli_inv/client/Exporter.java"
    ));

    #[test]
    fn java_process_builder_names_the_binary() {
        // `new ProcessBuilder("invctl", "export")`: argv[0] is the binary, the
        // subcommand rides the argv cell and never becomes a node of its own.
        for source in [
            JAVA_CLI_INV_EXPORTER,
            // Kotlin constructs without `new`.
            "val p = ProcessBuilder(\"invctl\", \"export\").inheritIO().start()",
        ] {
            let (qnames, argv) = invocation(source);
            assert_eq!(qnames, vec!["cli_invoke:invctl"], "{source}");
            assert_eq!(argv, vec![vec!["export"]], "{source}");
            assert!(!qnames.iter().any(|q| q == "cli_invoke:export"), "{source}");
        }
        // No literal argv, or the argv set later through `command(..)`: not read.
        assert!(
            invocation("Process p = new ProcessBuilder(cmd).start();")
                .0
                .is_empty()
        );
        assert!(
            invocation("new ProcessBuilder().command(\"invctl\").start();")
                .0
                .is_empty()
        );
    }

    #[test]
    fn java_process_builder_list_forms() {
        let (qnames, argv) =
            invocation("new ProcessBuilder(List.of(\"invctl\", \"export\", \"--all\")).start();");
        assert_eq!(qnames, vec!["cli_invoke:invctl"]);
        assert_eq!(argv, vec![vec!["export", "--all"]]);

        let (qnames, argv) =
            invocation("new ProcessBuilder(Arrays.asList(\"invctl\", \"import\"));");
        assert_eq!(qnames, vec!["cli_invoke:invctl"]);
        assert_eq!(argv, vec![vec!["import"]]);

        // Kotlin's list literal.
        let (qnames, argv) = invocation("ProcessBuilder(listOf(\"invctl\", \"sync\")).start()");
        assert_eq!(qnames, vec!["cli_invoke:invctl"]);
        assert_eq!(argv, vec![vec!["sync"]]);

        // A list built elsewhere is not read.
        assert!(
            invocation("new ProcessBuilder(List.of(tool, \"x\"));")
                .0
                .is_empty()
        );
        assert!(
            invocation("new ProcessBuilder(Arrays.asList(args));")
                .0
                .is_empty()
        );
    }

    #[test]
    fn java_runtime_exec_shell_string() {
        // The shell-string form is split on whitespace.
        let (qnames, argv) =
            invocation("Process p = Runtime.getRuntime().exec(\"invctl export --all\");");
        assert_eq!(qnames, vec!["cli_invoke:invctl"]);
        assert_eq!(argv, vec![vec!["export", "--all"]]);

        // The String[] overload (the only non-deprecated one since Java 18), in
        // both brace spellings, and Kotlin's `arrayOf`.
        for source in [
            "Runtime.getRuntime().exec(new String[]{\"invctl\", \"export\"});",
            "Runtime.getRuntime().exec(new String[] {\"invctl\", \"export\"});",
            "Runtime.getRuntime().exec(arrayOf(\"invctl\", \"export\"))",
        ] {
            let (qnames, argv) = invocation(source);
            assert_eq!(qnames, vec!["cli_invoke:invctl"], "{source}");
            assert_eq!(argv, vec![vec!["export"]], "{source}");
        }

        assert!(
            invocation("Runtime.getRuntime().exec(command);")
                .0
                .is_empty()
        );
    }

    // ---- DG.8: Scala (scopt, scala.sys.process) ------------------------------

    /// The matrix/scala/cli_def probe's scopt 4 builder, read from the fixture.
    const SCALA_CLI_DEF_MAIN: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/scala/cli_def/server/Main.scala"
    ));
    /// The matrix/scala/cli_inv probe's `scala.sys.process` launcher.
    const SCALA_CLI_INV_EXPORTER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/scala/cli_inv/client/Exporter.scala"
    ));

    #[test]
    fn scopt_builder_and_option_parser() {
        // scopt 4: `programName` is the binary, `cmd` a subcommand; an
        // `.action` lambda names no handler.
        let out = decls(SCALA_CLI_DEF_MAIN, "scala");
        assert_eq!(commands(&out), vec!["cli:invctl", "cli:export"]);
        assert_eq!(out.per_framework, vec![(CliFramework::Scopt, 2)]);
        assert!(out.refs.is_empty());
        assert_eq!(
            decl_marker("scala", &out, "Main.scala").as_deref(),
            Some("[cli-decl] scala=scopt:2 handler_refs=0 path=Main.scala")
        );

        // scopt 3: the OptionParser's constructor names the program; nested
        // `children(cmd(..))` are commands, `opt` / `head` are not.
        let v3 = "import scopt.OptionParser\nobject Cli {\n  val parser = new scopt.OptionParser[Config](\"shopctl\") {\n    head(\"shopctl\", \"1.x\")\n    opt[Int]('n', \"num\").action((x, c) => c.copy(n = x))\n    cmd(\"sync\").action((_, c) => c).children(cmd(\"orders\"), opt[Unit](\"dry-run\"))\n    cmd(\"gc\")\n  }\n}\n";
        assert_eq!(
            commands(&decls(v3, "scala")),
            vec!["cli:shopctl", "cli:sync", "cli:orders", "cli:gc"]
        );
        let extends =
            "import scopt._\nobject P extends OptionParser[Conf](\"tool\") { cmd(\"run\") }\n";
        assert_eq!(
            commands(&decls(extends, "scala")),
            vec!["cli:tool", "cli:run"]
        );
        // A qualified builder, without `import builder._`.
        let qualified = "import scopt.OParser\nval b = OParser.builder[C]\nval p = OParser.sequence(b.programName(\"x\"), b.cmd(\"y\"))\n";
        assert_eq!(commands(&decls(qualified, "scala")), vec!["cli:x", "cli:y"]);

        // `cmd(..)` in a Scala file that never mentions scopt is any method.
        assert!(
            commands(&decls(
                "object M { def cmd(s: String) = s; cmd(\"x\") }",
                "scala"
            ))
            .is_empty()
        );
        // Needles in strings and comments, interpolations and expressions.
        let guarded = "import scopt.OParser\nval help = \"programName(\\\"fake\\\")\"\n// cmd(\"line\")\n/* cmd(\"block\") */\nval a = cmd(s\"x$y\")\nval b = cmd(name)\nval c = OptionParser(\"no-type-arg\")\nval p = OParser.sequence(programName(\"real\"))\n";
        assert_eq!(commands(&decls(guarded, "scala")), vec!["cli:real"]);
        // The scan is Scala's: the same text in another language reads nothing.
        for lang in ["java", "kotlin", "python", "rust", "typescript"] {
            assert!(
                commands(&decls(SCALA_CLI_DEF_MAIN, lang)).is_empty(),
                "{lang}"
            );
        }
        // Every truncation of a multi-byte scopt source reads without a panic.
        let multibyte = "import scopt.OParser\nval p = new OptionParser[Ü](\"ä\") { cmd(\"é\").children(cmd(\"ö\")) }\nOParser.sequence(programName(\"ü\"), cmd(";
        for (cut, _) in multibyte.char_indices() {
            let _ = decls(&multibyte[..cut], "scala");
        }
        assert_eq!(
            commands(&decls(multibyte, "scala")),
            vec!["cli:ä", "cli:é", "cli:ö", "cli:ü"]
        );
    }

    #[test]
    fn scala_sys_process_invocations() {
        // `Seq(..).!`: argv[0] is the binary, `export` rides the argv cell and
        // never becomes a node of its own.
        let (qnames, argv) = invocation(SCALA_CLI_INV_EXPORTER);
        assert_eq!(qnames, vec!["cli_invoke:invctl"]);
        assert_eq!(argv, vec![vec!["export"]]);

        let cases: &[(&str, &str, &[&str])] = &[
            (
                "import sys.process._\nval s = \"git status\".!!",
                "git",
                &["status"],
            ),
            (
                "import scala.sys.process._\nProcess(Seq(\"docker\", \"build\")).run()",
                "docker",
                &["build"],
            ),
            (
                "import scala.sys.process._\nList(\"kubectl\", \"apply\").lazyLines",
                "kubectl",
                &["apply"],
            ),
            (
                "import scala.sys.process._\nVector[String](\"terraform\", \"plan\").!!",
                "terraform",
                &["plan"],
            ),
            (
                "import scala.sys.process._\nval rc = Seq(\"make\", \"test\") !\n",
                "make",
                &["test"],
            ),
            (
                "import scala.sys.process._\nSeq(\"helm\", \"lint\")\n  .!<",
                "helm",
                &["lint"],
            ),
            (
                "import scala.sys.process._\n\"git log\".lazyLines_!",
                "git",
                &["log"],
            ),
            (
                "import scala.sys.process.Process\nProcess(\"git\", Seq(\"fetch\")).!",
                "git",
                &["fetch"],
            ),
            (
                "import scala.sys.process._\nProcess.apply(\"ls -la\").!!",
                "ls",
                &["-la"],
            ),
            (
                "val code = scala.sys.process.Process(List(\"sbt\", \"compile\")).!",
                "sbt",
                &["compile"],
            ),
        ];
        for (source, bin, args) in cases {
            let (qnames, argv) = invocation(source);
            assert_eq!(qnames, vec![format!("cli_invoke:{bin}")], "{source}");
            assert_eq!(argv, vec![args.to_vec()], "{source}");
        }

        for source in [
            // A list that is never run, and a comparison.
            "import scala.sys.process._\nval xs = Seq(\"a\", \"b\").map(f)",
            "import scala.sys.process._\nif (Seq(\"a\", \"b\") != other) ()",
            "import scala.sys.process._\nval same = \"a\" != \"b\"",
            // An argv built elsewhere, an interpolated command, a commented one.
            "import scala.sys.process._\nval argv = Seq(\"invctl\", \"export\")\nargv.!",
            "import scala.sys.process._\nval log = s\"git log $rev\".!!",
            "import scala.sys.process._\n// Seq(\"rm\", \"-rf\").!\n/* \"rm -rf\".!! */",
            // No sys.process in code: the fixture's call without its import,
            // and an import only named in a comment or a string.
            "object Exporter {\n  def run(): Int = Seq(\"invctl\", \"export\").!\n}\n",
            "// uses sys.process\nval rc = Seq(\"invctl\", \"export\").!",
            "val doc = \"import scala.sys.process._\"\nval rc = Seq(\"invctl\", \"export\").!",
        ] {
            assert!(invocation(source).0.is_empty(), "{source}");
        }

        // Every truncation of a multi-byte launcher reads without a panic.
        let multibyte = "import scala.sys.process._\nval a = Seq(\"ä\", \"é\").!\nval b = \"ö ü\".!!\nProcess(\"ß\", List[String](\"ñ\")).run()\nVector(\"x";
        for (cut, _) in multibyte.char_indices() {
            let _ = invocation(&multibyte[..cut]);
        }
        assert_eq!(
            invocation(multibyte).0,
            vec!["cli_invoke:ß", "cli_invoke:ä", "cli_invoke:ö"]
        );
    }

    // ---- DH.10: PHP, Dart and Elixir launches --------------------------------

    /// The matrix/php/cli_inv probe's Symfony Process launcher.
    const PHP_CLI_INV_RUNNER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/php/cli_inv/client/Runner.php"
    ));
    /// The matrix/dart/cli_inv probe's dart:io launcher.
    const DART_CLI_INV_RUN: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/dart/cli_inv/client/bin/run.dart"
    ));
    /// The matrix/elixir/cli_inv probe's `System.cmd` launcher.
    const ELIXIR_CLI_INV_RUNNER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/elixir/cli_inv/client/lib/runner.ex"
    ));

    /// `(source, binary, argv)` cases each minting exactly that one node.
    fn assert_invocations(cases: &[(&str, &str, &[&str])]) {
        for (source, bin, args) in cases {
            let (qnames, argv) = invocation(source);
            assert_eq!(qnames, vec![format!("cli_invoke:{bin}")], "{source}");
            assert_eq!(argv, vec![args.to_vec()], "{source}");
        }
    }

    #[test]
    fn symfony_process_argv_array() {
        // `new Process([..])`: the argv array's first element is the binary;
        // `sync` rides the argv cell and never becomes a node of its own.
        let (qnames, argv) = invocation(PHP_CLI_INV_RUNNER);
        assert_eq!(qnames, vec!["cli_invoke:mytool"]);
        assert_eq!(argv, vec![vec!["sync"]]);
        assert_invocations(&[
            (
                "$p = new Process([\"mytool\", \"sync\", \"--force\"]);",
                "mytool",
                &["sync", "--force"],
            ),
            (
                "$p = Process::fromShellCommandline(\"mytool sync --all\");",
                "mytool",
                &["sync", "--all"],
            ),
        ]);
        // C#'s `new Process()` and a Process built from a variable: no site.
        assert!(invocation("var p = new Process();").0.is_empty());
        assert!(invocation("$p = new Process($argv);").0.is_empty());
    }

    /// GD.21: the sorted `cli_invoke:` qnames a file at `path` mints, with
    /// its residual counts.
    fn residual_invocations(source: &str, path: &str) -> (Vec<String>, (usize, usize)) {
        let (exec, fq) = if is_php_path(path) {
            let (e, f) = php_residual_sites(source);
            (e.len(), f.len())
        } else {
            (0, 0)
        };
        (invocation_qnames(source, path), (exec, fq))
    }

    #[test]
    fn php_global_exec_in_php_files_only() {
        let source = r#"<?php
function thumbnail(PDO $db, $ch): void
{
    exec('convert a.png b.jpg');
    \exec("ffmpeg -i in.mp4 out.webm", $out, $code);
    $db->exec('CREATE TABLE thumbs (id INTEGER)');
    $db?->exec('DROP TABLE thumbs');
    $r = PDO::exec('VACUUM');
    $exec('rogue arg');
    curl_exec($ch);
    $a = pcntl_exec('/bin/true');
    // exec('commented out');
    $s = 'exec("quoted")';
    echo "<script>re.exec('abc')</script>";
}
class Runner { public function exec($cmd) { return 1; } }
"#;
        let (qnames, counts) = residual_invocations(source, "src/run.php");
        assert_eq!(qnames, ["cli_invoke:convert", "cli_invoke:ffmpeg"]);
        assert_eq!(counts, (2, 0));
        let (_, argv) = invocation(source);
        assert!(argv.is_empty(), "no needle reads exec outside a .php path");
        // JavaScript: RegExp.exec and a bare exec are never launches.
        for path in ["js/x.js", "src/a.ts", "run.py", "run.phpx"] {
            let js = "export const hasA = (s) => /a/.exec(s) !== null;\nconst m = re.exec(\"abc\");\nexec('convert a.png b.jpg');\n";
            assert!(residual_invocations(js, path).0.is_empty(), "{path}");
        }
    }

    #[test]
    fn php_fully_qualified_symfony_process() {
        let source = r#"<?php
$a = new \Symfony\Component\Process\Process(['git', 'status']);
$b = new Symfony\Component\Process\Process(["make", "build"]);
$c = \Symfony\Component\Process\Process::fromShellCommandline('npm ci');
// $d = new \Symfony\Component\Process\Process(['commented']);
$e = \Symfony\Component\Process\Process(['no-new']);
$f = new \Symfony\Component\Process\Process($argv);
"#;
        let (qnames, counts) = residual_invocations(source, "app/Jobs/Build.php");
        assert_eq!(
            qnames,
            ["cli_invoke:git", "cli_invoke:make", "cli_invoke:npm"]
        );
        assert_eq!(counts, (0, 2));
        assert_eq!(
            Residuals {
                php_exec: 1,
                symfony_fq: 2,
                ..Residuals::default()
            }
            .marker("app/Jobs/Build.php")
            .as_deref(),
            Some(
                "[cli] php exec=1 symfony_fq=2 args_aliases=0 pubspec_exec=0 gemspec_concat=0 path=app/Jobs/Build.php"
            )
        );
        // Every truncation of a multi-byte PHP source reads without a panic.
        let multibyte = "<?php exec('é ö'); new \\Symfony\\Component\\Process\\Process(['ü']);";
        for (cut, _) in multibyte.char_indices() {
            let _ = php_residual_sites(&multibyte[..cut]);
        }
    }

    #[test]
    fn php_shell_exec_split() {
        assert_invocations(&[
            ("$out = shell_exec(\"mytool sync\");", "mytool", &["sync"]),
            (
                "passthru(\"mytool sync --verbose\");",
                "mytool",
                &["sync", "--verbose"],
            ),
            (
                "$h = proc_open(\"mytool sync\", $spec, $pipes);",
                "mytool",
                &["sync"],
            ),
            (
                "$h = proc_open([\"mytool\", \"sync\"], $spec, $pipes);",
                "mytool",
                &["sync"],
            ),
        ]);
    }

    #[test]
    fn dart_process_run_argv() {
        let (qnames, argv) = invocation(DART_CLI_INV_RUN);
        assert_eq!(qnames, vec!["cli_invoke:mytool"]);
        assert_eq!(argv, vec![vec!["sync"]]);
        assert_invocations(&[
            (
                "final r = Process.runSync(\"mytool\", [\"sync\"]);",
                "mytool",
                &["sync"],
            ),
            (
                "final p = await Process.start(\"mytool\", [\"sync\", \"--watch\"]);",
                "mytool",
                &["sync", "--watch"],
            ),
        ]);
        // C#'s spelling is its own needle, still read.
        assert_eq!(
            invocation("Process.Start(\"mytool\", \"sync\");").0,
            vec!["cli_invoke:mytool"]
        );
    }

    #[test]
    fn elixir_system_cmd_argv() {
        let (qnames, argv) = invocation(ELIXIR_CLI_INV_RUNNER);
        assert_eq!(qnames, vec!["cli_invoke:mytool"]);
        assert_eq!(argv, vec![vec!["sync"]]);
        assert_invocations(&[
            (
                "{out, 0} = System.cmd(\"mytool\", [\"sync\"], into: IO.stream())",
                "mytool",
                &["sync"],
            ),
            (
                "System.shell(\"mytool sync --all\")",
                "mytool",
                &["sync", "--all"],
            ),
        ]);
        assert!(invocation("System.cmd(tool, [\"sync\"])").0.is_empty());
    }

    #[test]
    fn regex_exec_is_not_an_invocation() {
        // A bare `exec(` is no needle: a regex match reads no binary.
        for source in [
            "const m = re.exec(\"abc\");",
            "if (/x+/.exec(\"xx\")) {}",
            "$r = $re->exec(\"abc\");",
        ] {
            assert!(invocation(source).0.is_empty(), "{source}");
        }
    }

    // ---- DH.10: Dart package:args and Elixir Mix tasks ------------------------

    /// The matrix/dart/cli_def probe's package:args runner, read from the fixture.
    const DART_CLI_DEF_MYTOOL: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/dart/cli_def/server/bin/mytool.dart"
    ));
    /// The matrix/elixir/cli_def probe's Mix task.
    const ELIXIR_CLI_DEF_TASK: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/elixir/cli_def/server/lib/mix/tasks/shop.sync.ex"
    ));

    #[test]
    fn dart_command_runner_and_command_names() {
        // The runner's first argument is the executable a caller types; the
        // Command subclass's `name` is a subcommand handled by its class.
        let out = decls(DART_CLI_DEF_MYTOOL, "dart");
        assert_eq!(commands(&out), vec!["cli:sync", "cli:mytool"]);
        assert_eq!(
            handlers(&out),
            vec![("cli:sync".to_string(), bare("SyncCommand"))]
        );
        assert_eq!(out.per_framework, vec![(CliFramework::CommandRunner, 2)]);
        assert_eq!(
            decl_marker("dart", &out, "bin/mytool.dart").as_deref(),
            Some("[cli-decl] dart=commandrunner:2 handler_refs=1 path=bin/mytool.dart")
        );
        // The scan is gated on `package:args/` and on the dart tag.
        let ungated = DART_CLI_DEF_MYTOOL.replace("package:args/", "package:other/");
        assert!(decls(&ungated, "dart").nodes.is_empty());
        for lang in ["typescript", "java", "kotlin", "python", "ruby"] {
            assert!(
                commands(&decls(DART_CLI_DEF_MYTOOL, lang)).is_empty(),
                "{lang}"
            );
        }
    }

    #[test]
    fn dart_getter_name_form() {
        let source = r#"import 'package:args/command_runner.dart' as args;

class DeployCommand extends args.Command<int> {
  @override
  String get name => 'deploy';
  @override
  String get description => 'Deploy';
}

class GcCommand extends Command {
  String get name {
    return "gc";
  }
}

class PlainCommand extends BaseCommand {
  @override
  final String name = 'plain';
}

class Runner extends CommandRunner<void> {
  Runner() : super('shopctl', 'The shop tool');
}

final runner = new CommandRunner("legacy", "Old style");
"#;
        let out = decls(source, "dart");
        assert_eq!(
            commands(&out),
            vec![
                "cli:deploy",
                "cli:gc",
                "cli:plain",
                "cli:shopctl",
                "cli:legacy"
            ]
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:deploy".to_string(), bare("DeployCommand")),
                ("cli:gc".to_string(), bare("GcCommand")),
                ("cli:plain".to_string(), bare("PlainCommand")),
            ]
        );

        let nothing = r#"import 'package:args/command_runner.dart';

// final name = 'commented';
/* CommandRunner('blocked', 'x') */
final help = "CommandRunner('quoted', 'x')";
final raw = r'CommandRunner("raw", "x")';

abstract class BaseCommand extends Command<void> {
  String get name;
}

class LocalCommand extends Command<void> {
  @override
  void run() {
    final name = 'local';
    print(name);
  }
  String get name => '$prefix-sync';
}

class NotACommand extends Widget {
  final name = 'widget';
}

class Mixed = Object with Command;
final runner = CommandRunner(executable, 'built');
"#;
        let out = decls(nothing, "dart");
        assert!(out.nodes.is_empty(), "{:?}", commands(&out));
        assert_eq!(decl_marker("dart", &out, "bin/x.dart"), None);

        // Every truncation of a multi-byte source reads without a panic.
        let multibyte = "import 'package:args/args.dart';\nclass XCommand extends Command { final name = 'é'; }\nCommandRunner<Ü>('ö', 'ß')..run(";
        for (cut, _) in multibyte.char_indices() {
            let _ = decls(&multibyte[..cut], "dart");
        }
        assert_eq!(commands(&decls(multibyte, "dart")), vec!["cli:é", "cli:ö"]);
    }

    #[test]
    fn dart_command_aliases_are_commands() {
        let source = r#"import 'package:args/command_runner.dart';

class SyncCommand extends Command<void> {
  @override
  final name = 'sync';
  @override
  List<String> get aliases => const ['pull-env', "s"];
}

class GcCommand extends Command {
  String get name => 'gc';
  @override
  final List<String> aliases = const <String>['clean'];
}

class LsCommand extends Command {
  String get name => 'ls';
  List<String> get aliases {
    return ['list', '$prefix-ls', dir];
  }
}

class NoNameCommand extends Command {
  List<String> get aliases => ['orphan'];
}

class LocalCommand extends Command {
  String get name => 'local';
  void run() {
    final aliases = ['inner'];
  }
  List<String> get aliases => computeAliases();
}
"#;
        let out = decls(source, "dart");
        assert_eq!(
            commands(&out),
            vec![
                "cli:sync",
                "cli:pull-env",
                "cli:s",
                "cli:gc",
                "cli:clean",
                "cli:ls",
                "cli:list",
                "cli:local"
            ]
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:sync".to_string(), bare("SyncCommand")),
                ("cli:pull-env".to_string(), bare("SyncCommand")),
                ("cli:s".to_string(), bare("SyncCommand")),
                ("cli:gc".to_string(), bare("GcCommand")),
                ("cli:clean".to_string(), bare("GcCommand")),
                ("cli:ls".to_string(), bare("LsCommand")),
                ("cli:list".to_string(), bare("LsCommand")),
                ("cli:local".to_string(), bare("LocalCommand")),
            ]
        );
        assert_eq!(out.residuals.args_aliases, 4);
        assert_eq!(
            decl_marker("dart", &out, "bin/tool.dart").as_deref(),
            Some(
                "[cli-decl] dart=commandrunner:8 handler_refs=8 path=bin/tool.dart\n\
                 [cli] php exec=0 symfony_fq=0 args_aliases=4 pubspec_exec=0 gemspec_concat=0 path=bin/tool.dart"
            )
        );
        // A file with no alias keeps its one-line marker.
        let plain = decls(DART_CLI_DEF_MYTOOL, "dart");
        assert_eq!(plain.residuals, Residuals::default());
    }

    #[test]
    fn pubspec_executables_keys() {
        let pubspec = "name: envtool # the package\nenvironment:\n  sdk: ^3.0.0\nexecutables:\n  # the default script\n  envtool:\n\n  envcheck: check\n  \"quoted\": q\nflutter:\n  uses-material-design: true\n";
        assert_eq!(
            scan_pubspec_executables(pubspec),
            ["envtool", "envcheck", "quoted"]
        );
        assert_eq!(
            scan_pubspec_executables("name: x\nexecutables: {a: , b: c}\n"),
            ["a", "b"]
        );
        for none in [
            "name: x\n",
            "name: x\nexecutables:\nflutter:\n",
            "name: x\nexecutables: ~\n",
            "name: x\ndev:\n  executables:\n    nested:\n",
        ] {
            assert!(scan_pubspec_executables(none).is_empty(), "{none}");
        }
        let mut nodes = Vec::new();
        let mut nav = CodeNav::default();
        extract_pubspec_executables(
            pubspec,
            "tools/pubspec.yaml",
            module_id(),
            repo(),
            &mut nodes,
            &mut nav,
        );
        let out = CliNodes {
            nodes,
            nav,
            ..CliNodes::default()
        };
        assert_eq!(
            manifest_commands(&out),
            ["cli:envtool", "cli:envcheck", "cli:quoted"]
        );
        let mut nodes = Vec::new();
        let mut nav = CodeNav::default();
        extract_pubspec_executables(
            pubspec,
            "tools/x.yaml",
            module_id(),
            repo(),
            &mut nodes,
            &mut nav,
        );
        assert!(nodes.is_empty());
    }

    #[test]
    fn mix_task_names() {
        let out = decls(ELIXIR_CLI_DEF_TASK, "elixir");
        assert_eq!(commands(&out), vec!["cli:shop.sync"]);
        assert_eq!(
            handlers(&out),
            vec![("cli:shop.sync".to_string(), bare("Sync"))]
        );
        assert_eq!(out.per_framework, vec![(CliFramework::MixTask, 1)]);
        assert_eq!(
            decl_marker("elixir", &out, "lib/mix/tasks/shop.sync.ex").as_deref(),
            Some("[cli-decl] elixir=mix:1 handler_refs=1 path=lib/mix/tasks/shop.sync.ex")
        );

        for (module, task, handler) in [
            ("Ecto.Gen.Repo", "ecto.gen.repo", "Repo"),
            ("HTTPServer", "http_server", "HTTPServer"),
            ("ShopSync", "shop_sync", "ShopSync"),
            ("Phx2.Gen", "phx2.gen", "Gen"),
        ] {
            let source = format!(
                "defmodule Mix.Tasks.{module} do\n  use Mix.Task\n  def run(_), do: :ok\nend\n"
            );
            let out = decls(&source, "elixir");
            assert_eq!(commands(&out), vec![format!("cli:{task}")], "{module}");
            assert_eq!(
                handlers(&out),
                vec![(format!("cli:{task}"), bare(handler))],
                "{module}"
            );
        }
        // A compiler is a task too.
        let compiler = "defmodule Mix.Tasks.Compile.Assets do\n  use Mix.Task.Compiler\n  def run(_), do: {:ok, []}\nend\n";
        assert_eq!(
            commands(&decls(compiler, "elixir")),
            vec!["cli:compile.assets"]
        );

        // Elixir's Macro.underscore, segment by segment.
        for (seg, want) in [
            ("Sync", "sync"),
            ("ShopSync", "shop_sync"),
            ("HTTPServer", "http_server"),
            ("Foo2Bar", "foo2_bar"),
            ("FOOBar", "foo_bar"),
            ("A", "a"),
            ("V2", "v2"),
        ] {
            assert_eq!(macro_underscore(seg), want, "{seg}");
        }
    }

    #[test]
    fn mix_task_needs_use_mix_task() {
        let source = r#"defmodule Mix.Tasks.Helper do
  # use Mix.Task
  def helper, do: "use Mix.Task"
end

defmodule Mix.Tasks.Docs.Example do
  @moduledoc """
  defmodule Mix.Tasks.Fake do
    use Mix.Task
  end
  """
  use Mix.TaskHelper
end

defmodule MyApp.Tasks.Sync do
  use Mix.Task
end

defmodule Mix.Tasks.Real do
  @quote ?"
  use Mix.Task
  def run(_), do: :ok
end
"#;
        let out = decls(source, "elixir");
        assert_eq!(commands(&out), vec!["cli:real"]);
        assert_eq!(handlers(&out), vec![("cli:real".to_string(), bare("Real"))]);
        // Other languages never run the Mix scan.
        assert!(decls(ELIXIR_CLI_DEF_TASK, "ruby").nodes.is_empty());
        // Every truncation reads without a panic.
        let multibyte = "defmodule Mix.Tasks.Ünï do\n  use Mix.Task\n  @doc ?é\nend\ndefmodule Mix.Tasks.Ok do use Mix.Task end";
        for (cut, _) in multibyte.char_indices() {
            let _ = decls(&multibyte[..cut], "elixir");
        }
        assert_eq!(commands(&decls(multibyte, "elixir")), vec!["cli:ok"]);
    }

    #[test]
    fn argv_cell_round_trips_and_ignores_other_cells() {
        let argv = vec![vec!["migrate".to_string(), "--yes".to_string()]];
        assert_eq!(parse_argv_cell(&argv_cell("mytool", &argv)), argv);
        let text = Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text("x".into()),
        };
        assert!(parse_argv_cell(&text).is_empty());
    }

    // ---- LA.20c: binaries declared by packaging manifests -----------------

    fn manifest_bins(source: &str, path: &str) -> CliNodes {
        extract_manifest_binaries(source, path, module_id(), repo())
    }

    /// GH.20: `(name, manifest, file, line, target)` of every command's
    /// manifest-bin cell, in emission order.
    /// `(name, manifest, file, line, target)`.
    type BinRow = (String, String, Option<String>, u32, String);

    fn bin_targets(out: &CliNodes) -> Vec<BinRow> {
        out.nodes
            .iter()
            .flat_map(|n| {
                let name = out.nav.name_by_id.get(&n.id).cloned().unwrap_or_default();
                n.cells
                    .iter()
                    .filter_map(parse_manifest_bin_cell)
                    .map(move |t| (name.clone(), t.manifest, t.file, t.line, t.target))
            })
            .collect()
    }

    fn bt(name: &str, form: &str, file: Option<&str>, line: u32, target: &str) -> BinRow {
        (
            name.into(),
            form.into(),
            file.map(str::to_string),
            line,
            target.into(),
        )
    }

    #[test]
    fn manifest_bins_carry_their_targets() {
        let cargo = "[package]\nname = \"svc\"\n\n[[bin]]\nname = \"api\"\npath = \"src/bin/api.rs\"\n\n[[bin]]\nname = \"seed\"\n\n[[bin]]\nname = \"svc\"\n";
        assert_eq!(
            bin_targets(&manifest_bins(cargo, "svc/Cargo.toml")),
            [
                bt("api", "cargo", Some("svc/Cargo.toml"), 4, "src/bin/api.rs"),
                bt(
                    "seed",
                    "cargo",
                    Some("svc/Cargo.toml"),
                    8,
                    "src/bin/seed.rs"
                ),
                bt("svc", "cargo", Some("svc/Cargo.toml"), 11, "src/main.rs"),
            ]
        );
        let pyproject = "[project]\nname = \"x\"\n\n[project.scripts]\naliases = \"aliases:cli\"\nextra = \"pkg.cli:main [fancy]\"\n\n[tool.poetry.scripts]\npoe = { callable = \"pkg.poe:run\" }\nold = { reference = \"pkg.old:go\", type = \"console\" }\nfile = { reference = \"bin/run.sh\", type = \"file\" }\n";
        let py = Some("examples/a/pyproject.toml");
        assert_eq!(
            bin_targets(&manifest_bins(pyproject, "examples/a/pyproject.toml")),
            [
                bt("aliases", "pyproject", py, 4, "aliases:cli"),
                bt("extra", "pyproject", py, 5, "pkg.cli:main [fancy]"),
                bt("poe", "pyproject", py, 8, "pkg.poe:run"),
                bt("old", "pyproject", py, 9, "pkg.old:go"),
            ],
            "a `type = \"file\"` script names no object reference"
        );
        let npm = "{\n  \"name\": \"@acme/tool\",\n  \"bin\": {\n    \"tool\": \"./bin/tool.js\",\n    \"t2\": \"bin/t2.js\"\n  }\n}\n";
        assert_eq!(
            bin_targets(&manifest_bins(npm, "package.json")),
            [
                bt("t2", "npm", Some("package.json"), 4, "bin/t2.js"),
                bt("tool", "npm", Some("package.json"), 3, "./bin/tool.js"),
            ]
        );
        let one = "{\"name\": \"@acme/solo\", \"bin\": \"cli.js\"}";
        assert_eq!(
            bin_targets(&manifest_bins(one, "web/package.json")),
            [bt("solo", "npm", Some("web/package.json"), 0, "cli.js")]
        );
        let composer = "{\n  \"bin\": [\"bin/mytool\"]\n}\n";
        assert_eq!(
            bin_targets(&manifest_bins(composer, "composer.json")),
            [bt(
                "mytool",
                "composer",
                Some("composer.json"),
                1,
                "bin/mytool"
            )]
        );
        let gemspec = "Gem::Specification.new do |spec|\n  spec.bindir = \"exe\"\n  spec.executables = [\"mytool\"]\nend\n";
        assert_eq!(
            bin_targets(&manifest_bins(gemspec, "mytool.gemspec")),
            [bt(
                "mytool",
                "gemspec",
                Some("mytool.gemspec"),
                2,
                "exe/mytool"
            )]
        );
        let default_bindir = "Gem::Specification.new do |s|\n  s.executables << \"x\"\nend\n";
        assert_eq!(
            bin_targets(&manifest_bins(default_bindir, "x.gemspec")),
            [bt("x", "gemspec", Some("x.gemspec"), 1, "bin/x")]
        );
    }

    #[test]
    fn setup_py_bins_carry_targets_without_a_file() {
        let src = "from setuptools import setup\nsetup(\n    entry_points={\n        'console_scripts': [\n            'mytool = mytool.cli:main',\n        ],\n    },\n)\n";
        assert_eq!(
            bin_targets(&decls(src, "python")),
            [bt("mytool", "setup", None, 4, "mytool.cli:main")]
        );
        let ini = "from setuptools import setup\nsetup(\n    entry_points='''\n    [console_scripts]\n    inimytool = pkg.cli:main\n    [paste.app_factory]\n    main = pkg:app\n    ''',\n)\n";
        assert_eq!(
            bin_targets(&decls(ini, "python")),
            [bt("inimytool", "setup", None, 4, "pkg.cli:main")]
        );
        let one = "setup(entry_points={'console_scripts': 'solo = pkg:main'})\n";
        assert_eq!(
            bin_targets(&decls(one, "python")),
            [bt("solo", "setup", None, 0, "pkg:main")]
        );
    }

    #[test]
    fn manifest_bin_cell_round_trips_and_is_no_argv_cell() {
        let cell = manifest_bin_cell("cargo", Some("Cargo.toml"), 3, "src/main.rs");
        assert_eq!(
            cell.payload,
            CellPayload::Json(
                r#"{"manifest":"cargo","file":"Cargo.toml","line":3,"target":"src/main.rs"}"#
                    .into()
            )
        );
        let t = parse_manifest_bin_cell(&cell).expect("parses");
        assert_eq!(
            (
                t.manifest.as_str(),
                t.file.as_deref(),
                t.line,
                t.target.as_str()
            ),
            ("cargo", Some("Cargo.toml"), 3, "src/main.rs")
        );
        assert!(parse_argv_cell(&cell).is_empty());
        assert!(parse_manifest_bin_cell(&argv_cell("x", &[vec!["a".into()]])).is_none());
        assert_eq!(
            manifest_bin_cell("setup", None, 0, "m:f").payload,
            CellPayload::Json(r#"{"manifest":"setup","line":0,"target":"m:f"}"#.into())
        );
    }

    /// Every node is a CLI_COMMAND parented to the manifest MODULE; returns
    /// the qnames in emission order.
    fn manifest_commands(out: &CliNodes) -> Vec<String> {
        for n in &out.nodes {
            assert_eq!(out.nav.kind_by_id.get(&n.id), Some(&node_kind::CLI_COMMAND));
            assert_eq!(out.nav.parent_of.get(&n.id), Some(&module_id()));
        }
        assert!(out.refs.is_empty(), "a manifest emits no HANDLED_BY");
        commands(out)
    }

    /// The matrix/python/cli_def probe's pyproject.toml, read from the fixture.
    const PY_CLI_DEF_PYPROJECT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/python/cli_def/server/pyproject.toml"
    ));

    #[test]
    fn pyproject_project_scripts_declare_binaries() {
        let out = manifest_bins(PY_CLI_DEF_PYPROJECT, "server/pyproject.toml");
        // `[project] name = "mytool"` is the distribution, not a script: one node.
        assert_eq!(manifest_commands(&out), ["cli:mytool"]);
        assert_eq!(
            manifest_marker("pyproject.toml", &out).as_deref(),
            Some(
                "[cli-decl] manifest bins=1 (pyproject=1 setup=0 npm=0 cargo=0 composer=0 gemspec=0) path=pyproject.toml"
            )
        );

        let pyproject = "[project]\nname = \"acme\"\ndependencies = [\"click\"]\n\n\
[project.scripts]\n# the main tool\nacme = \"acme.cli:main\"\n\"acme-admin\" = 'acme.admin:main'\n\n\
[ project . \"gui-scripts\" ]\nacme-gui = \"acme.gui:run\"\n\n\
[tool.black]\nline-length = 100\n";
        let out = manifest_bins(pyproject, "pyproject.toml");
        assert_eq!(
            manifest_commands(&out),
            ["cli:acme", "cli:acme-admin", "cli:acme-gui"]
        );
    }

    #[test]
    fn poetry_scripts_and_multi_line_inline_tables() {
        let pyproject = "[tool.poetry]\nname = \"svc\"\n\n\
[tool.poetry.scripts]\nsvc = \"svc.main:run\"\nsvc-file = { reference = \"bin/svc.sh\", type = \"file\" }\n\
svc-long = {\n  reference = \"svc.long:run\",\n  type = \"console\"\n}\nsvc-after = \"svc.after:run\"\n\n\
[tool.poetry.dependencies]\npython = \"^3.11\"\n";
        let out = manifest_bins(pyproject, "pyproject.toml");
        // The continuation lines `reference = ...` / `type = ...` are not keys.
        assert_eq!(
            manifest_commands(&out),
            ["cli:svc", "cli:svc-file", "cli:svc-long", "cli:svc-after"]
        );
        // A pyproject with no script table declares no binary.
        assert!(
            manifest_bins("[project]\nname = \"lib\"\n", "pyproject.toml")
                .nodes
                .is_empty()
        );
    }

    #[test]
    fn package_json_bin_object_and_scoped_string() {
        let obj = r#"{"name": "@acme/shipit", "bin": {"shipit": "bin/cli.js", "ship-admin": "bin/admin.js"}}"#;
        let out = manifest_bins(obj, "server/package.json");
        // serde_json's map is sorted by key; `@acme/shipit` is never minted.
        assert_eq!(manifest_commands(&out), ["cli:ship-admin", "cli:shipit"]);
        assert_eq!(
            manifest_marker("server/package.json", &out).as_deref(),
            Some(
                "[cli-decl] manifest bins=2 (pyproject=0 setup=0 npm=2 cargo=0 composer=0 gemspec=0) path=server/package.json"
            )
        );

        let scoped = r#"{"name": "@acme/deployer", "bin": "./cli.js"}"#;
        assert_eq!(
            manifest_commands(&manifest_bins(scoped, "package.json")),
            ["cli:deployer"]
        );
        let plain = r#"{"name": "deployer", "bin": "./cli.js"}"#;
        assert_eq!(
            manifest_commands(&manifest_bins(plain, "package.json")),
            ["cli:deployer"]
        );

        // No `bin`, or not JSON: nothing, and no marker.
        let lib = manifest_bins(r#"{"name": "lib", "main": "index.js"}"#, "package.json");
        assert!(lib.nodes.is_empty());
        assert_eq!(manifest_marker("package.json", &lib), None);
        assert!(manifest_bins("{ not json", "package.json").nodes.is_empty());
    }

    #[test]
    fn cargo_bin_tables_only() {
        let cargo = "[package]\nname = \"tools\"\nversion = \"0.1.0\"\n\n\
[[bin]]\nname = \"migrate\" # the db tool\npath = \"src/bin/migrate.rs\"\n\n\
[[bin]]\npath = \"src/bin/seed.rs\"\nname = 'seed'\n\n\
[[example]]\nname = \"demo\"\n\n[dependencies]\nname = \"not-a-bin\"\n";
        let out = manifest_bins(cargo, "Cargo.toml");
        assert_eq!(manifest_commands(&out), ["cli:migrate", "cli:seed"]);
        assert_eq!(
            manifest_marker("Cargo.toml", &out).as_deref(),
            Some(
                "[cli-decl] manifest bins=2 (pyproject=0 setup=0 npm=0 cargo=2 composer=0 gemspec=0) path=Cargo.toml"
            )
        );
        // A package with no [[bin]] is not assumed to be a binary: whether
        // src/main.rs exists is not visible to a per-file extractor.
        let lib = "[package]\nname = \"tools\"\n\n[dependencies]\nserde = \"1\"\n";
        assert!(manifest_bins(lib, "Cargo.toml").nodes.is_empty());
    }

    #[test]
    fn non_manifest_paths_yield_nothing() {
        let scripts = "[project.scripts]\nx = \"x:main\"\n";
        assert!(manifest_bins(scripts, "setup.cfg").nodes.is_empty());
        assert!(
            manifest_bins(scripts, "docs/pyproject.toml.txt")
                .nodes
                .is_empty()
        );
        assert_eq!(
            manifest_marker("setup.py", &manifest_bins(scripts, "pyproject.toml")),
            None
        );
    }

    #[test]
    fn setup_py_console_scripts_forms() {
        let setup = r#"from setuptools import setup

setup(
    name="acme",
    entry_points={
        "console_scripts": [
            "acme = acme.cli:main",  # the tool
            'acme-admin=acme.admin:main',
        ],
        "gui_scripts": ["acme-gui = acme.gui:run"],
    },
)
"#;
        let out = decls(setup, "python");
        assert_eq!(manifest_commands(&out), ["cli:acme", "cli:acme-admin"]);
        assert_eq!(
            decl_marker("python", &out, "setup.py").as_deref(),
            Some(
                "[cli-decl] manifest bins=2 (pyproject=0 setup=2 npm=0 cargo=0 composer=0 gemspec=0) path=setup.py"
            )
        );

        let kwarg = "from setuptools import setup\nsetup(entry_points=dict(console_scripts=['tool = pkg.mod:func']))\n";
        assert_eq!(commands(&decls(kwarg, "python")), ["cli:tool"]);

        let ini = "from setuptools import setup\nsetup(\n    entry_points='''\n    [console_scripts]\n    inimytool = pkg.cli:main\n    [paste.app_factory]\n    main = pkg:app\n    ''',\n)\n";
        assert_eq!(commands(&decls(ini, "python")), ["cli:inimytool"]);

        let one_string = "setup(entry_points={'console_scripts': 'solo = pkg:main'})\n";
        assert_eq!(commands(&decls(one_string, "python")), ["cli:solo"]);
    }

    #[test]
    fn console_scripts_lookups_and_comments_declare_nothing() {
        let lookups = r#"from importlib.metadata import entry_points

# console_scripts = ["commented = x:y"]
eps = entry_points(group="console_scripts")
scripts = eps["console_scripts"]
note = "console_scripts: ['in = a:string']"
"#;
        let out = decls(lookups, "python");
        assert!(out.nodes.is_empty(), "{:?}", commands(&out));
        assert_eq!(decl_marker("python", &out, "tools.py"), None);
        // Other languages never run the setuptools scan.
        assert!(
            decls("x = {'console_scripts': ['a = b:c']}", "ruby")
                .nodes
                .is_empty()
        );
        // A malformed entry is not a binary.
        let bad =
            "setup(entry_points={'console_scripts': ['no-target =', '= x:y', 'two words = a:b']})";
        assert!(decls(bad, "python").nodes.is_empty());
    }

    #[test]
    fn click_and_console_scripts_in_one_file_keep_the_framework_line() {
        let both = r#"import click
from setuptools import setup

@click.command("sync")
def sync():
    pass

setup(entry_points={"console_scripts": ["mytool = mytool.cli:cli"]})
"#;
        let out = decls(both, "python");
        assert_eq!(commands(&out), ["cli:sync", "cli:mytool"]);
        assert_eq!(
            decl_marker("python", &out, "setup.py").as_deref(),
            Some("[cli-decl] python=click:1,manifest:1 handler_refs=0 path=setup.py")
        );
    }

    // ---- DH.10: composer.json `bin`, `*.gemspec` executables -------------------

    /// The matrix/php/cli_inv probe's composer.json.
    const PHP_CLI_INV_COMPOSER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/php/cli_inv/server/composer.json"
    ));
    /// The matrix/ruby/cli_inv probe's gemspec.
    const RUBY_CLI_INV_GEMSPEC: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../bench/substrate-gap/matrix/ruby/cli_inv/server/mytool.gemspec"
    ));

    #[test]
    fn composer_bin_basenames() {
        assert_eq!(
            manifest_commands(&manifest_bins(PHP_CLI_INV_COMPOSER, "composer.json")),
            ["cli:mytool"]
        );
        let one = r#"{"name": "acme/tool", "bin": "bin/tool"}"#;
        assert_eq!(
            manifest_commands(&manifest_bins(one, "server/composer.json")),
            ["cli:tool"]
        );
        let many = r#"{"bin": ["bin/a", "scripts/b.php", "bin/a", 7]}"#;
        assert_eq!(
            manifest_commands(&manifest_bins(many, "composer.json")),
            ["cli:a", "cli:b.php"]
        );
        // No `bin`, a `bin` of another shape, or not JSON: nothing.
        for source in [
            r#"{"name": "acme/lib", "require": {"php": "^8.2"}}"#,
            r#"{"bin": {"x": "y"}}"#,
            "{ nope",
        ] {
            assert!(
                manifest_bins(source, "composer.json").nodes.is_empty(),
                "{source}"
            );
        }
    }

    #[test]
    fn gemspec_executables_forms() {
        assert_eq!(
            manifest_commands(&manifest_bins(RUBY_CLI_INV_GEMSPEC, "mytool.gemspec")),
            ["cli:mytool"]
        );
        let gemspec = r#"# frozen_string_literal: true
Gem::Specification.new do |spec|
  spec.name = "toolkit"
  spec.executables = %w[alpha beta]
  spec.executables << 'gamma'
  spec.executables += ["zeta"] if true
  spec.executable = "delta"
  spec.executables.concat(%w(epsilon))
  spec.executables.concat ["eta", 'theta']
  spec.executables.concat(extra)
  spec.executables.push("not-read")
end
"#;
        // GD.21: `+=` and `.concat(..)` / `.concat [..]` append names too
        // (before, `+=` and `concat` read nothing); a variable reads nothing.
        assert_eq!(
            manifest_commands(&manifest_bins(gemspec, "toolkit.gemspec")),
            [
                "cli:alpha",
                "cli:beta",
                "cli:gamma",
                "cli:zeta",
                "cli:delta",
                "cli:epsilon",
                "cli:eta",
                "cli:theta"
            ]
        );
        assert_eq!(scan_gemspec_executables(gemspec).1, 4);
        assert_eq!(
            Residuals {
                gemspec_concat: 4,
                ..Residuals::default()
            }
            .marker("toolkit.gemspec")
            .as_deref(),
            Some(
                "[cli] php exec=0 symfony_fq=0 args_aliases=0 pubspec_exec=0 gemspec_concat=4 path=toolkit.gemspec"
            )
        );
        assert_eq!(Residuals::default().marker("x.gemspec"), None);
        let multi = "Gem::Specification.new do |s|\n  s.executables = [\n    \"one\", # the first\n    'two',\n    name,\n  ]\n  s.executables = %w(three)\n  s.executables = \"four\"\nend\n";
        assert_eq!(
            manifest_commands(&manifest_bins(multi, "pkg/multi.gemspec")),
            ["cli:one", "cli:two", "cli:three", "cli:four"]
        );
    }

    #[test]
    fn gemspec_glob_reads_nothing() {
        let source = r#"Gem::Specification.new do |spec|
  spec.bindir = "exe"
  spec.executables = spec.files.grep(%r{^exe/}) { |f| File.basename(f) }
  # spec.executables = ["commented"]
  executables = ["local"]
  puts "spec.executables = ['quoted']"
  spec.executables == ["compared"]
  spec.executables = ["built" + suffix]
  spec.executables = Dir["exe/*"].map { |f| File.basename(f) }
end
"#;
        let out = manifest_bins(source, "x.gemspec");
        assert!(out.nodes.is_empty(), "{:?}", commands(&out));
        assert_eq!(manifest_marker("x.gemspec", &out), None);
        // Every truncation of a multi-byte gemspec reads without a panic.
        let multibyte = "Gem::Specification.new do |s|\n  s.executables = %w[ä é]\n  s.executables << 'ö'\n  s.executables = [\"ü\", 'ß']\nend\n";
        for (cut, _) in multibyte.char_indices() {
            let _ = manifest_bins(&multibyte[..cut], "m.gemspec");
        }
        // Not a binary name: nothing is minted.
        assert!(manifest_bins(multibyte, "m.gemspec").nodes.is_empty());
    }

    #[test]
    fn manifest_marker_counts_composer_and_gemspec() {
        let out = manifest_bins(PHP_CLI_INV_COMPOSER, "composer.json");
        assert_eq!(
            manifest_marker("composer.json", &out).as_deref(),
            Some(
                "[cli-decl] manifest bins=1 (pyproject=0 setup=0 npm=0 cargo=0 composer=1 gemspec=0) path=composer.json"
            )
        );
        let out = manifest_bins(RUBY_CLI_INV_GEMSPEC, "server/mytool.gemspec");
        assert_eq!(
            manifest_marker("server/mytool.gemspec", &out).as_deref(),
            Some(
                "[cli-decl] manifest bins=1 (pyproject=0 setup=0 npm=0 cargo=0 composer=0 gemspec=1) path=server/mytool.gemspec"
            )
        );
        // A gemspec that ships no executable prints nothing.
        let lib = manifest_bins(
            "Gem::Specification.new { |s| s.name = \"lib\" }\n",
            "lib.gemspec",
        );
        assert_eq!(manifest_marker("lib.gemspec", &lib), None);
    }
}
