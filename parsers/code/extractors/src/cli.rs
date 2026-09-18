use repo_graph_code_domain::{
    CallQualifier, CodeNav, GRAPH_TYPE, UnresolvedRef, cell_type, edge_category, node_kind,
};
use repo_graph_core::{Cell, CellPayload, Confidence, Node, NodeId, RepoId};

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
}

/// One command a scanner read: the name a user types, and the symbol that
/// implements it when the declaration says.
#[derive(Debug, PartialEq)]
struct CliDecl {
    framework: CliFramework,
    name: String,
    handler: Option<CallQualifier>,
}

impl CliDecl {
    fn new(framework: CliFramework, name: String, handler: Option<CallQualifier>) -> Self {
        CliDecl { framework, name, handler }
    }
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
/// `java` (which also carries `.kt`); System.CommandLine / Spectre.Console.Cli
/// for `csharp`. LA.20b: Typer / click decorators and argparse subparsers for
/// `python` (commander's needle no longer reads Python, so a non-decorator
/// `db.command("ping")` mints nothing); Thor for `ruby`; Symfony Console and
/// Laravel artisan for `php`.
pub fn extract_cli_command_nodes(
    source: &str,
    lang: &str,
    module_id: NodeId,
    repo: RepoId,
) -> CliNodes {
    let mut decls: Vec<CliDecl> = Vec::new();

    let line_extractors: Vec<(LineExtractor, CliFramework)> = match lang {
        "go" => vec![(extract_cobra_command_name as LineExtractor, CliFramework::Cobra)],
        "typescript" | "react" | "angular" | "vue" => {
            vec![(extract_commander_command_name as LineExtractor, CliFramework::Commander)]
        }
        "rust" => vec![(extract_clap_command_name as LineExtractor, CliFramework::Clap)],
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
        "java" if source.contains("picocli") => {
            decls.extend(scan_picocli(source, &CodeMap::new(source)));
        }
        "csharp" if source.contains("System.CommandLine") || source.contains("Spectre.Console.Cli") => {
            decls.extend(scan_dotnet_cli(source, &CodeMap::new(source)));
        }
        "python"
            if source.contains(".command") || source.contains("add_typer") || source.contains("argparse") =>
        {
            let code = CodeMap::script(source, Script::Python);
            decls.extend(scan_typer_click(source, &code));
            decls.extend(scan_argparse(source, &code));
        }
        "ruby" if source.contains("Thor") => {
            decls.extend(scan_thor(source, &CodeMap::script(source, Script::Ruby)));
        }
        "php" if source.contains("Command") => {
            decls.extend(scan_symfony_laravel(source, &CodeMap::script(source, Script::Php)));
        }
        _ => {}
    }

    let mut out = CliNodes::default();
    let mut minted: std::collections::HashMap<String, NodeId> = std::collections::HashMap::new();
    let mut handled: Vec<(NodeId, CallQualifier)> = Vec::new();
    for decl in decls {
        let id = match minted.get(&decl.name) {
            Some(&id) => id,
            None => {
                let id = add_cli_command(&mut out.nodes, &mut out.nav, &decl.name, module_id, repo);
                minted.insert(decl.name.clone(), id);
                match out.per_framework.iter_mut().find(|(f, _)| *f == decl.framework) {
                    Some((_, n)) => *n += 1,
                    None => out.per_framework.push((decl.framework, 1)),
                }
                id
            }
        };
        if let Some(handler) = decl.handler {
            handled.push((id, handler));
        }
    }
    let mut bound: Vec<(NodeId, CallQualifier)> = Vec::new();
    for (id, handler) in handled {
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
            });
        }
    }
    out.per_framework.sort();
    out
}

/// LA.20a fired_on: `[cli-decl] rust=clap:3 handler_refs=0 path=src/main.rs`,
/// or `None` when the file declared no command. Only frameworks that fired are
/// listed; the prefix and `path=` are stable.
pub fn decl_marker(lang: &str, out: &CliNodes, path: &str) -> Option<String> {
    if out.per_framework.is_empty() {
        return None;
    }
    let counts: Vec<String> =
        out.per_framework.iter().map(|(f, n)| format!("{}:{n}", f.label())).collect();
    Some(format!(
        "[cli-decl] {lang}={} handler_refs={} path={path}",
        counts.join(","),
        out.refs.len()
    ))
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
        (from..to.min(b.len())).find(|&j| b[j] == b'\'').map(|j| j + 1)
    };
    match *b.get(at + 1)? {
        b'\\' => close_within(at + 3, at + 12),
        b'\'' => None,
        c if c.is_ascii() => (b.get(at + 2) == Some(&b'\'')).then_some(at + 3),
        // One multi-byte char: only non-ASCII bytes up to the closing quote.
        _ => close_within(at + 2, at + 6).filter(|&end| b[at + 1..end - 1].iter().all(|c| !c.is_ascii())),
    }
}

/// Where a scanner may match: every byte outside string literals, char
/// literals and comments. Built once per file by one lexing pass over
/// `"..."` (with `\` escapes; also Rust `b"..."`), Rust raw strings
/// (`r"..."`, `r#"..."#`, `br#"..."#`), Java / C# `"""..."""` blocks, C#
/// verbatim strings (`@"..."`, `""` escapes), char literals, `//` comments and
/// nesting `/* */` comments. A needle in a doc comment or a string (a test
/// fixture, a scanner's own needle table) is not a declaration.
struct CodeMap {
    /// `[start, end)` of each literal / comment, in source order.
    skips: Vec<(usize, usize)>,
}

impl CodeMap {
    fn new(source: &str) -> Self {
        let b = source.as_bytes();
        let mut skips = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let prev = |k: usize| i.checked_sub(k).map(|p| b[p]);
            let end = match b[i] {
                b'/' if b.get(i + 1) == Some(&b'/') => Some(skip_comment(b, i)),
                b'/' if b.get(i + 1) == Some(&b'*') => Some(skip_nested_block_comment(b, i)),
                b'"' if b[i..].starts_with(b"\"\"\"") => Some(
                    find_bytes(b, i + 3, b"\"\"\"").map_or(b.len(), |j| j + 3),
                ),
                b'"' if prev(1) == Some(b'@') || (prev(1) == Some(b'$') && prev(2) == Some(b'@')) => {
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
    fn is_code(&self, pos: usize) -> bool {
        let i = self.skips.partition_point(|&(start, _)| start <= pos);
        i == 0 || self.skips[i - 1].1 <= pos
    }
}

/// First index at or after `from` where `needle` starts.
fn find_bytes(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    b.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| from + p)
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
        if b[k] == b'"' && b.get(k + 1..k + 1 + hashes).is_some_and(|h| h.iter().all(|&c| c == b'#')) {
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
    if b.get(j).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') {
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
    let args = if i < close && b.get(i) == Some(&b'(') { attr_span(source, i) } else { None };
    Some(RustAttr { name, args, end: close + 1 })
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
    if matches!(attr.name, "command" | "clap") { attr.args } else { None }
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
        let rule = attrs.iter().filter_map(clap_command_args).find_map(|a| attr_kv(a, "rename_all"));
        let Some(open) = source.get(after_kw..).and_then(|s| s.find('{')).map(|r| after_kw + r)
        else {
            continue;
        };
        let Some((body_start, body_end)) = balanced(source, open, MAX_ENUM_BODY) else { continue };
        for (variant, vattrs) in enum_variants(source, body_start, body_end) {
            let vargs: Vec<&str> = vattrs.iter().filter_map(clap_command_args).collect();
            let unnamed = |a: &&str| {
                attr_flag(a, "flatten") || attr_flag(a, "external_subcommand") || attr_flag(a, "skip")
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
            let Some((_, close)) = balanced(source, j, MAX_ENUM_BODY) else { break };
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
    for part in ident.split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()) {
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
        Some(first) => first.to_uppercase().chain(chars.flat_map(char::to_lowercase)).collect(),
        None => String::new(),
    }
}

/// A variant's command name under clap's `rename_all` rule (default
/// kebab-case). The rule string is normalised the way clap_derive does it, so
/// `snake_case`, `snake` and `SnakeCase` are one rule; an unknown rule falls
/// back to the default.
fn clap_rename(variant: &str, rule: Option<&str>) -> String {
    let words = heck_words(variant);
    let lower = |sep: &str| words.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>().join(sep);
    let upper = |sep: &str| words.iter().map(|w| w.to_uppercase()).collect::<Vec<_>>().join(sep);
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
            .map(|(i, w)| if i == 0 { w.to_lowercase() } else { capitalise(w) })
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
    let process_command =
        uses.iter().any(|u| u.contains("process::") && !word_positions(u, "Command").is_empty());
    let bare_command = lists("clap::", "Command") && !process_command;
    let bare_app = lists("clap::", "App");
    let mut out = Vec::new();
    for (needle, bare_ok) in
        [("Command::new(", bare_command), ("App::new(", bare_app), ("SubCommand::with_name(", true)]
    {
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
                bare_ok && !prefix.bytes().last().is_some_and(|c| is_ident_byte(c) || c == b'.')
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
        let Some(span) = attr_span(source, open) else { continue };
        let close = open + 1 + span.len();
        from = close;
        let Some(name) = attr_kv(span, "name") else { continue };
        let handler = picocli_handler(source, code, close + 1, at);
        out.push(CliDecl::new(CliFramework::Picocli, name, handler));
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
                Some(class) => CallQualifier::Attribute { base: class.to_string(), name: method },
                None => CallQualifier::Bare(method),
            });
        }
        // Generic / array / nullable return types: `List<String> run(`.
        while matches!(b.get(i), Some(b'<' | b'>' | b'[' | b']' | b',' | b'?' | b'.')) {
            i = skip_trivia(b, i + 1);
        }
    }
    None
}

/// The class whose declaration most recently precedes `at` (`class Tool`).
fn enclosing_class<'a>(source: &'a str, code: &CodeMap, at: usize) -> Option<&'a str> {
    let head = source.get(..at)?;
    word_positions(head, "class").into_iter().rev().filter(|&p| code.is_code(p)).find_map(|p| {
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
    for at in word_positions(source, "new").into_iter().filter(|&at| code.is_code(at)) {
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
        let Some(name) = source.get(first..).and_then(extract_quoted) else { continue };
        let handler =
            assigned_variable(source, at).and_then(|v| set_handler_target(source, code, v));
        out.push(CliDecl::new(CliFramework::SystemCommandLine, name, handler));
    }
}

/// The variable the expression at `at` is assigned to: `var sync = ` /
/// `Command sync = ` -> `sync`. Not for `==`, `+=`, `=>` and friends.
fn assigned_variable(source: &str, at: usize) -> Option<&str> {
    let head = source.get(..at)?.trim_end().strip_suffix('=')?;
    if head.ends_with(['=', '!', '<', '>', '+', '-', '*', '/', '%', '&', '|', '^', '?']) {
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
    word_positions(source, var).into_iter().filter(|&at| code.is_code(at)).find_map(|at| {
        let after_var = at + var.len();
        let rest = source.get(after_var..)?.strip_prefix('.')?;
        let method = ["SetHandler(", "SetAction("].into_iter().find(|m| rest.starts_with(m))?;
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
            let Some(name) = source.get(skip_trivia(b, open + 1)..).and_then(extract_quoted) else {
                continue;
            };
            let handler = if handled { ty.map(|t| CallQualifier::Bare(t.to_string())) } else { None };
            out.push(CliDecl::new(CliFramework::SpectreCli, name, handler));
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
enum Script {
    Python,
    Ruby,
    Php,
}

impl CodeMap {
    /// [`CodeMap::new`]'s contract for Python, Ruby and PHP, whose literals
    /// and comments are not the C family's: `#` line comments (not PHP 8's
    /// `#[` attribute opener), PHP's `//` and `/* */`, Ruby's `=begin` /
    /// `=end`, Python's `'''` / `"""` blocks, `'...'` strings of any length (in
    /// the C family `'` opens a char literal), and heredoc bodies (Ruby `<<~ID`
    /// / `<<-ID`, PHP `<<<ID`) through their terminator line. Ruby `%w()`
    /// literals are read as code.
    fn script(source: &str, script: Script) -> Self {
        let b = source.as_bytes();
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
            let end = match b[i] {
                b'#' if script == Script::Php && b.get(i + 1) == Some(&b'[') => None,
                b'#' => Some(line_end(b, i)),
                b'/' if script == Script::Php && b.get(i + 1) == Some(&b'/') => Some(line_end(b, i)),
                b'/' if script == Script::Php && b.get(i + 1) == Some(&b'*') => Some(skip_comment(b, i)),
                q @ (b'"' | b'\'') if script == Script::Python && b[i..].starts_with(&[q, q, q]) => {
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

    /// `pos` when it is code, else the end of the literal or comment holding it.
    fn next_code(&self, pos: usize) -> usize {
        let i = self.skips.partition_point(|&(start, _)| start <= pos);
        match i.checked_sub(1).map(|k| self.skips[k]) {
            Some((_, end)) if end > pos => end,
            _ => pos,
        }
    }
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
    let quote = b.get(i).copied().filter(|c| matches!(c, b'"' | b'\'' | b'`'));
    if quote.is_some() {
        i += 1;
    }
    let start = i;
    if !b.get(i).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_') {
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
fn balanced_code(source: &str, code: &CodeMap, open_at: usize, cap: usize) -> Option<(usize, usize)> {
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
fn kwarg_str(source: &str, code: &CodeMap, (start, end): (usize, usize), key: &str, sep: u8) -> Option<String> {
    let b = source.as_bytes();
    code_words(source, code, start, end, key).into_iter().find_map(|at| {
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
        && !name.contains(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '$' | '{' | '}' | '\\'))
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
    let names = |item: &str| item.trim().split(|c: char| c == '.' || c.is_whitespace()).next() == Some(module);
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
        source.contains(&format!("{obj} = typer.Typer(")) || source.contains(&format!("{obj} = Typer("))
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
            let word = ["def ", "class "].into_iter().find_map(|kw| rest.strip_prefix(kw));
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
    b[..at].iter().rev().take_while(|&&c| c != b'\n').all(|&c| c == b' ' || c == b'\t')
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
        let Some(obj) = source.get(at + 1..name_end).and_then(|d| d.strip_suffix(".command")) else {
            continue;
        };
        if last != "command" {
            continue;
        }
        let framework = py_cli_framework(source, obj, typer, click);
        let (explicit, end) = if b.get(skip_ws(b, name_end)) == Some(&b'(') {
            let Some(span) = call_args(source, code, name_end) else { continue };
            (py_explicit_name(source, code, span), span.1 + 1)
        } else if click {
            (None, name_end)
        } else {
            continue;
        };
        from = end;
        let func = py_decorated_function(source, code, end);
        let Some(name) = py_command_name(explicit, func, framework, typer || click, &defs) else { continue };
        let handler = func.map(|f| CallQualifier::Bare(f.to_string()));
        found.push((at, CliDecl::new(framework, name, handler)));
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
            let Some(span) = call_args(source, code, at + "command".len()) else { continue };
            let Some(target) = call_args(source, code, span.1 + 1) else { continue };
            let Some(segs) = dotted_exact(source, target) else { continue };
            let framework = py_cli_framework(source, obj, typer, click);
            let func = segs.last().copied();
            let explicit = py_explicit_name(source, code, span);
            let Some(name) = py_command_name(explicit, func, framework, true, &defs) else { continue };
            found.push((at, CliDecl::new(framework, name, callable_qualifier(&segs))));
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
fn py_set_defaults_handler(source: &str, code: &CodeMap, (start, end): (usize, usize)) -> Option<CallQualifier> {
    let b = source.as_bytes();
    ["func", "handler"].into_iter().find_map(|key| {
        code_words(source, code, start, end, key).into_iter().find_map(|at| {
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
        .chain(code_words(source, code, 0, len, "add_parser").into_iter().map(|at| (at, false)))
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
            let name =
                first_positional_str(source, span).or_else(|| kwarg_str(source, code, span, "name", b'='))?;
            (!defs.contains(name.as_str())).then_some((span, name))
        });
        let minted = name.map(|(span, name)| {
            let chained = source
                .get(span.1 + 1..)
                .filter(|rest| rest.starts_with(".set_defaults"))
                .and_then(|_| call_args(source, code, span.1 + 1 + ".set_defaults".len()))
                .and_then(|defaults| py_set_defaults_handler(source, code, defaults));
            out.push(CliDecl::new(CliFramework::Argparse, name, chained));
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
        let Some(&(_, _, Some(idx))) = vars.iter().rev().find(|(pos, v, _)| *pos < at && *v == var) else {
            continue;
        };
        if let Some(decl) = out.get_mut(idx)
            && decl.handler.is_none()
            && let Some(span) = call_args(source, code, at + "set_defaults".len())
        {
            decl.handler = py_set_defaults_handler(source, code, span);
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
    let after = parent.strip_prefix("::").unwrap_or(parent).strip_prefix("Thor")?;
    if after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == ':') {
        return None;
    }
    let last = name.trim().rsplit("::").next()?;
    (last.starts_with(|c: char| c.is_ascii_uppercase()) && last.bytes().all(is_ident_byte)).then_some(last)
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
        let Some(class) = thor_class(&line[indent..]) else { continue };
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
                    pending = ruby_first_string(rest).and_then(|(usage, _)| usage.split_whitespace().next());
                }
                "subcommand" => {
                    let Some((name, tail)) = ruby_first_string(rest) else { continue };
                    if !valid_cli_name(name) {
                        continue;
                    }
                    let klass = tail.trim_start().strip_prefix(',').map(str::trim_start).and_then(|k| {
                        let end = k.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':'));
                        k[..end.unwrap_or(k.len())].rsplit("::").next().filter(|s| !s.is_empty())
                    });
                    let handler = klass.map(|k| CallQualifier::Bare(k.to_string()));
                    out.push(CliDecl::new(CliFramework::Thor, name.to_string(), handler));
                }
                "def" => {
                    let (method, end) = read_ident(t, skip_ws(t.as_bytes(), after));
                    let singleton_or_op = matches!(t.as_bytes().get(end), Some(b'.' | b'?' | b'!' | b'='));
                    if method.is_empty() || singleton_or_op || method == "initialize" || private {
                        continue;
                    }
                    let Some(usage) = pending.take() else { continue };
                    let name = if usage.replace('-', "_") == method { usage } else { method };
                    if valid_cli_name(name) {
                        let handler =
                            CallQualifier::Attribute { base: class.to_string(), name: method.to_string() };
                        out.push(CliDecl::new(CliFramework::Thor, name.to_string(), Some(handler)));
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
        let Some(open) = (after..limit).find(|&i| b[i] == b'{' && code.is_code(i)) else { continue };
        let console = code_words(source, code, after, open, "extends").first().is_some_and(|&x| {
            let start = skip_ws(b, x + "extends".len());
            let mut end = start;
            while end < open && (is_ident_byte(b[end]) || b[end] == b'\\') {
                end += 1;
            }
            source[start..end].rsplit('\\').next().is_some_and(|t| t.ends_with("Command"))
        });
        let body = balanced_code(source, code, open, MAX_CLASS_BODY).unwrap_or((open + 1, b.len()));
        out.push(PhpClass { name, body, console });
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
    let mut push = |at: usize, framework: CliFramework, names: &str, aliases: bool, class: Option<&str>| {
        let names: Vec<&str> = if aliases { names.split('|').collect() } else { vec![names] };
        for name in names.into_iter().filter(|n| valid_cli_name(n)) {
            let handler = class.map(|c| CallQualifier::Bare(c.to_string()));
            found.push((at, CliDecl::new(framework, name.to_string(), handler)));
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
        let Some(span) = call_args(source, code, at + "AsCommand".len()) else { continue };
        let Some(names) = first_positional_str(source, span).or_else(|| kwarg_str(source, code, span, "name", b':'))
        else {
            continue;
        };
        let class = balanced_code(source, code, head - 1, MAX_ATTR_SPAN)
            .and_then(|(_, close)| php_attribute_target(source, code, close + 1));
        push(at, CliFramework::Symfony, &names, true, class);
    }
    for class in classes.iter().filter(|c| c.console) {
        let (start, end) = class.body;
        for (var, framework) in
            [("$defaultName", CliFramework::Symfony), ("$signature", CliFramework::Laravel), ("$name", CliFramework::Laravel)]
        {
            for at in code_words(source, code, start, end, var) {
                if !matches!(php_prev_word(source, at), "public" | "protected" | "private" | "static" | "var" | "string") {
                    continue;
                }
                let eq = skip_ws(b, at + var.len());
                if b.get(eq) != Some(&b'=') || matches!(b.get(eq + 1), Some(b'=' | b'>')) {
                    continue;
                }
                let Some(lit) = source.get(skip_ws(b, eq + 1)..end).and_then(quoted_literal) else { continue };
                let name = if var == "$signature" {
                    let lit = lit.trim_start();
                    &lit[..lit.find(|c: char| c.is_whitespace() || c == '{').unwrap_or(lit.len())]
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

    CliNodes { nodes, nav, ..CliNodes::default() }
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
        out.nodes.iter().filter_map(|n| out.nav.qname_by_id.get(&n.id).cloned()).collect()
    }

    /// Every HANDLED_BY ref as (command qname, handler).
    fn handlers(out: &CliNodes) -> Vec<(String, CallQualifier)> {
        out.refs
            .iter()
            .map(|r| {
                assert_eq!(r.category, edge_category::HANDLED_BY);
                assert_eq!(r.from_module, module_id());
                (out.nav.qname_by_id.get(&r.from).cloned().unwrap_or_default(), r.qualifier.clone())
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
        assert_eq!(commands(&out), vec!["cli:gc", "cli:shopctl", "cli:sync-users"]);
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
        assert_eq!(commands(&out), vec!["cli:sync_users", "cli:gc", "cli:import"]);
    }

    #[test]
    fn clap_parser_enum_variants_are_commands() {
        let source = "use clap::Parser;\n#[derive(Parser)]\nenum Cli { Build, RunAll(RunArgs) }\n";
        assert_eq!(commands(&decls(source, "rust")), vec!["cli:build", "cli:run-all"]);
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
        assert_eq!(clap_rename("SyncUsers", Some("SCREAMING_SNAKE_CASE")), "SYNC_USERS");
        assert_eq!(clap_rename("SyncUsers", Some("no-such-rule")), "sync-users");
    }

    #[test]
    fn clap_builder_names_but_std_process_command_does_not() {
        let builder = "use clap::{Arg, Command};\nfn cli() -> Command {\n    Command::new(\"shopctl\").subcommand(Command::new(\"migrate\"))\n}\n";
        assert_eq!(commands(&decls(builder, "rust")), vec!["cli:shopctl", "cli:migrate"]);

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
        assert_eq!(commands(&out), vec!["cli:invctl", "cli:purge", "cli:list", "cli:export"]);
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
        // Kotlin rides the java arm.
        let kt = "import picocli.CommandLine.Command\n@Command(name = \"kt\")\nclass KtTool : Runnable { override fun run() {} }\n";
        assert_eq!(handlers(&decls(kt, "java")), vec![("cli:kt".to_string(), bare("KtTool"))]);
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
        assert_eq!(commands(&out), vec!["cli:reconcile", "cli:export", "cli:audit"]);
        assert_eq!(
            handlers(&out),
            vec![
                ("cli:reconcile".to_string(), bare("Reconcile")),
                (
                    "cli:audit".to_string(),
                    CallQualifier::Attribute { base: "Handlers".into(), name: "Audit".into() }
                ),
            ]
        );
        assert_eq!(out.per_framework, vec![(CliFramework::SystemCommandLine, 3)]);
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
        let typer = "import typer\napp = typer.Typer()\n@app.command(\"purge\")\ndef purge(): ...\n";
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
            assert!(commands(&decls("runner.command(\"y\")", lang)).is_empty(), "{lang}");
        }
        for lang in ["typescript", "react", "angular", "vue"] {
            assert_eq!(commands(&decls("program.command('y')", lang)), vec!["cli:y"], "{lang}");
        }
        // Cobra is Go-only; clap's line form is Rust-only.
        assert!(commands(&decls("x := &cobra.Command{Use: \"m\"}", "typescript")).is_empty());
        assert!(commands(&decls("#[command(name = \"n\")]", "python")).is_empty());
        assert!(commands(&decls("anything", "swift")).is_empty());
    }

    #[test]
    fn attr_span_is_bounded_quote_aware_and_char_safe() {
        let s = "#[command(name = \"é\", about = \"naïve ) (\", c = ')')] x";
        assert_eq!(attr_span(s, 9), Some("name = \"é\", about = \"naïve ) (\", c = ')'"));
        assert_eq!(attr_span(s, 0), None, "not an opening bracket");
        assert_eq!(attr_span(s, s.len() + 10), None, "out of range");
        assert_eq!(attr_span("(\"unterminated)", 0), None);
        assert_eq!(attr_span("(a]", 0), None, "mismatched closer");
        assert_eq!(attr_span("(a // )\n)", 0), Some("a // )\n"));
        let long = format!("({})", "é".repeat(MAX_ATTR_SPAN));
        assert_eq!(attr_span(&long, 0), None, "over the cap");
        assert_eq!(attr_kv("about = \"name = 'x'\", name = \"real\"", "name").as_deref(), Some("real"));
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
            ("use clap::Command; fn f() { Command::new(\"ä\"); } #[derive(Subcommand)] enum E { Ä, B }", "rust"),
            ("import picocli.X; @Command(name = \"é\") class Ü { @Command(name=\"m\") void ä() {} }", "java"),
            ("using System.CommandLine; var ü = new Command(\"é\"); ü.SetHandler(Ä); using Spectre.Console.Cli; c.AddCommand<Ö>(\"x\"); c.AddBranch<", "csharp"),
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
        CallQualifier::Attribute { base: base.to_string(), name: name.to_string() }
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
        assert_eq!(handlers(&out), vec![("cli:rollout".to_string(), attr("Deployer", "rollout"))]);
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
            vec!["cli:rollout-all", "cli:status", "cli:download", "cli:next_public", "cli:db"]
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
        assert_eq!(commands(&out), vec!["cli:app:sync-orders", "cli:app:legacy-import"]);
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
        assert_eq!(commands(&out), vec!["cli:app:create-user", "cli:app:add-user", "cli:app:configured"]);
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
        assert_eq!(handlers(&out), vec![("cli:emails:send".to_string(), bare("SendEmails"))]);
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
        assert_eq!(commands(&out), vec!["cli:import-users", "cli:purge", "cli:dbtool", "cli:migrate"]);
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
        assert_eq!(commands(&out), vec!["cli:users", "cli:list", "cli:sync-now", "cli:serve", "cli:run-all"]);
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
        assert_eq!(commands(&out), vec!["cli:init-db", "cli:export-users", "cli:seed"]);
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
        assert_eq!(handlers(&out), vec![("cli:seed".to_string(), bare("migrate"))]);
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
        assert_eq!(commands(&out), vec!["cli:init", "cli:serve", "cli:check", "cli:lazy"]);
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
            ("class Ü < Thor\n  desc \"é ENV\", \"ü\"\n  def é; end\n  subcommand \"ö\", Ä\nend\n", "ruby"),
            ("<?php #[AsCommand(name: 'é|ü')] class Ä extends Command { protected $signature = 'ö {x}'; } <<<É\n", "php"),
            ("import typer, argparse\n@app.command()\ndef é(): ...\nm = s.add_parser(\"ü\")\nm.set_defaults(func=ö)\n", "python"),
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
