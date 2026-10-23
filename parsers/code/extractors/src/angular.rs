//! Angular component/service/directive/pipe/guard/module extraction.
//!
//! Pattern-based, runs per-file on TS under Angular-detected projects. Emits:
//!   - COMPONENT: class with `@Component({...})` decorator.
//!   - SERVICE: class with `@Injectable({...})` decorator.
//!   - DIRECTIVE: class with `@Directive({...})` decorator.
//!   - PIPE: class with `@Pipe({...})` decorator.
//!   - GUARD: classes ending in `Guard` (CanActivate/CanDeactivate patterns).
//!
//! Angular Router tables (`{ path: 'users', component: UsersComponent,
//! children: [...] }`) are the shared route-table walker's
//! (`crate::nav_routes`, LA.6b).

use glia_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use glia_core::{Confidence, Node, NodeId, NodeKindId, RepoId};

pub struct AngularNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
}

pub fn extract_angular_nodes(
    source: &str,
    module_qname: &str,
    module_id: NodeId,
    repo: RepoId,
) -> AngularNodes {
    let mut nodes = Vec::new();
    let mut nav = CodeNav::default();

    for (name, kind) in scan_decorated_classes(source) {
        let qname = format!("{module_qname}::{name}");
        let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
        nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells: Vec::new(),
        });
        nav.record(id, &name, &qname, kind, Some(module_id));
    }

    AngularNodes { nodes, nav }
}

fn scan_decorated_classes(source: &str) -> Vec<(String, NodeKindId)> {
    let mut out: Vec<(String, NodeKindId)> = Vec::new();
    // Match each `@<Decorator>(` occurrence; balance-skip its parenthesised
    // metadata (which routinely spans many lines for `@Component({selector,
    // templateUrl, imports, ...})`); then look for the next `class <Name>`
    // declaration. Line-based state machines fail here because Angular
    // decorator metadata is multi-line by convention.
    const DECORATORS: &[(&str, NodeKindId)] = &[
        ("@Component(", node_kind::COMPONENT),
        ("@Injectable(", node_kind::SERVICE),
        ("@Directive(", node_kind::DIRECTIVE),
        ("@Pipe(", node_kind::PIPE),
    ];

    for &(needle, kind) in DECORATORS {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            let arg_start = pos + needle.len();
            let close = match find_balanced_paren(&source[arg_start..]) {
                Some(off) => arg_start + off,
                None => {
                    search_from = arg_start;
                    continue;
                }
            };
            // After `)`, scan forward until we find `class <Name>`. Allow any
            // amount of whitespace, `export`, `default`, `abstract`, comments.
            if let Some(name) = find_next_class_name(&source[close + 1..]) {
                out.push((name, kind));
            }
            search_from = close + 1;
        }
    }

    // Convention-based: classes ending in `Guard` without a matching @ decorator
    // get GUARD kind.
    for line in source.lines() {
        let t = line.trim_start();
        if let Some(name) = extract_class_name(t)
            && name.ends_with("Guard")
            && !out.iter().any(|(n, _)| n == &name)
        {
            out.push((name, node_kind::GUARD));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.0.cmp(&b.1.0)));
    out.dedup();
    out
}

/// One `@Component(` / `@Directive(` decorator and what its metadata says
/// about the class's bindings (DT.5a): the inline `template:` and the `host:`
/// entries, read as text, for [`crate::angular_template::bind_inline`]; and
/// (DT.5b) the `templateUrl:` an external template is named by, which the
/// engine's post-cache graft reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecoratedClass {
    /// The decorated class's simple name; `None` when no `class <Name>`
    /// follows the decorator.
    pub name: Option<String>,
    /// COMPONENT or DIRECTIVE.
    pub kind: NodeKindId,
    /// The `template:` value when it is ONE string literal.
    pub template: Option<InlineTemplate>,
    /// DT.5b: the `templateUrl:` value when it is ONE string literal, as
    /// written (`./orders.page.html`), with the 0-based row of the literal in
    /// the file. A non-literal value reads as `None`.
    pub template_url: Option<(String, u32)>,
    /// A `template:` key whose value is no single literal (a concatenation,
    /// an identifier, a call): read as nothing.
    pub template_unread: bool,
    /// The `host:` object's entries with a quoted value, in source order.
    pub host: Vec<HostEntry>,
    /// DT.5c: the class body's members Angular itself invokes, bound by a
    /// member decorator ([`MemberDecorator`]), in source order.
    pub members: Vec<DecoratedMember>,
}

/// DT.5c: a member decorator whose member Angular calls: on a host event
/// (`@HostListener`), to read a host binding (`@HostBinding`), on an input
/// write (`@Input`) or a view / content query update (`@ViewChild`,
/// `@ViewChildren`, `@ContentChild`, `@ContentChildren`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberDecorator {
    HostListener,
    HostBinding,
    Input,
    Query,
}

impl MemberDecorator {
    /// The decorator named `name` (`HostListener`, `ViewChild`, ..).
    fn named(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"HostListener" => Self::HostListener,
            b"HostBinding" => Self::HostBinding,
            b"Input" => Self::Input,
            b"ViewChild" | b"ViewChildren" | b"ContentChild" | b"ContentChildren" => Self::Query,
            _ => return None,
        })
    }

    /// The EVIDENCE rule of the USES edge the bound member gets.
    pub fn rule(self) -> &'static str {
        match self {
            Self::HostListener => "host_listener",
            Self::HostBinding => "host_binding",
            Self::Input => "input_setter",
            Self::Query => "query_setter",
        }
    }

    /// The member shapes Angular invokes through this decorator: a
    /// `@HostListener` method, a `@HostBinding` getter or method, an `@Input`
    /// or query setter. A decorated field is none of them.
    fn accepts(self, shape: MemberShape) -> bool {
        matches!(
            (self, shape),
            (Self::HostListener, MemberShape::Method)
                | (Self::HostBinding, MemberShape::Getter | MemberShape::Method)
                | (Self::Input | Self::Query, MemberShape::Setter)
        )
    }
}

/// DT.5c: one decorator-bound member of a decorated class: the decorator,
/// the member's simple name (a getter / setter is its property name) and the
/// 0-based row of the decorator's `@` in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecoratedMember {
    pub decorator: MemberDecorator,
    pub name: String,
    pub row: u32,
}

/// The class member that follows a member decorator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemberShape {
    /// `name(` (or `name<T>(`).
    Method,
    /// `get name(`.
    Getter,
    /// `set name(`.
    Setter,
}

/// An inline template literal: its raw content (escapes kept, so offsets stay
/// the file's) and the byte offset of its first char in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineTemplate {
    pub text: String,
    pub start: usize,
}

/// One `host:` entry (`'(document:keydown.escape)': 'onEscape()'`,
/// `class: 'x'`): its key without quotes, its quoted value's content and the
/// key's 0-based row in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEntry {
    pub key: String,
    pub value: String,
    pub row: u32,
}

/// Every `@Component(` / `@Directive(` decorator of `source`, in source order
/// (DT.5a). The argument is balanced through [`find_balanced_paren`] and the
/// class named by [`find_next_class_name`], as [`scan_decorated_classes`]
/// does; inside the argument's object only first-level `template` / `host`
/// keys are read. Text-level, UTF-8 safe: every slice is cut at an ASCII
/// delimiter.
pub fn decorated_classes(source: &str) -> Vec<DecoratedClass> {
    const DECORATORS: &[(&str, NodeKindId)] = &[
        ("@Component(", node_kind::COMPONENT),
        ("@Directive(", node_kind::DIRECTIVE),
    ];
    let mut found: Vec<(usize, DecoratedClass)> = Vec::new();
    // DT.5c: the source with `//` and `/* */` comments blanked (offsets and
    // newlines kept), read for the class bodies' member decorators.
    let mut masked: Option<Vec<u8>> = None;
    for &(needle, kind) in DECORATORS {
        let mut from = 0;
        while let Some(rel) = source[from..].find(needle) {
            let pos = from + rel;
            let arg = pos + needle.len();
            let Some(off) = find_balanced_paren(&source[arg..]) else {
                from = arg;
                continue;
            };
            let close = arg + off;
            let class = next_class(&source[close + 1..]);
            let b = masked.get_or_insert_with(|| mask_ts_comments(source.as_bytes()));
            let members = class
                .as_ref()
                .and_then(|(_, at)| class_body(b, close + 1 + at))
                .map(|body| member_decorators(b, source, body))
                .unwrap_or_default();
            let mut dc = DecoratedClass {
                name: class.map(|(name, _)| name),
                kind,
                template: None,
                template_url: None,
                template_unread: false,
                host: Vec::new(),
                members,
            };
            read_metadata(source, arg, close, &mut dc);
            found.push((pos, dc));
            from = close + 1;
        }
    }
    found.sort_by_key(|(pos, _)| *pos);
    found.into_iter().map(|(_, dc)| dc).collect()
}

/// The first-level `template` / `templateUrl` / `host` keys of the object
/// literal in `source[start..end]` (a decorator's argument).
fn read_metadata(source: &str, start: usize, end: usize, dc: &mut DecoratedClass) {
    let b = source.as_bytes();
    let i = skip_trivia(b, start, end);
    if b.get(i) != Some(&b'{') {
        return;
    }
    for (key, vs, ve) in object_entries(b, i + 1, end) {
        match key.as_str() {
            "template" => match single_literal(b, vs, ve) {
                Some((s, e)) => {
                    dc.template = Some(InlineTemplate {
                        text: source[s..e].to_string(),
                        start: s,
                    });
                    dc.template_unread = false;
                }
                None => dc.template_unread = dc.template.is_none(),
            },
            // DT.5b: the external template's path, as written. A repeated key
            // keeps the last literal, as the object literal itself does.
            "templateUrl" => {
                if let Some((s, e)) = single_literal(b, vs, ve) {
                    dc.template_url = Some((
                        source[s..e].to_string(),
                        glia_code_domain::line_of(source, s),
                    ));
                }
            }
            "host" if b.get(vs) == Some(&b'{') => {
                for (hk, hvs, hve) in object_entries(b, vs + 1, ve) {
                    if let Some((s, e)) = single_literal(b, hvs, hve) {
                        dc.host.push(HostEntry {
                            key: hk.key,
                            value: source[s..e].to_string(),
                            row: glia_code_domain::line_of(source, hk.at),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

/// An object key and the byte offset it starts at.
struct Key {
    key: String,
    at: usize,
}

impl Key {
    fn as_str(&self) -> &str {
        &self.key
    }
}

/// The `key: value` entries of the object whose body starts at `i` (past
/// its `{`) and ends at its matching `}` or `end`: each key (quoted or bare)
/// and its value's trimmed byte range. A shorthand entry (`{ template }`)
/// has an empty range; a spread or computed key is skipped.
fn object_entries(b: &[u8], mut i: usize, end: usize) -> Vec<(Key, usize, usize)> {
    let mut out = Vec::new();
    loop {
        i = skip_trivia(b, i, end);
        while i < end && b[i] == b',' {
            i = skip_trivia(b, i + 1, end);
        }
        if i >= end || b[i] == b'}' {
            return out;
        }
        let at = i;
        let key = match b[i] {
            q @ (b'\'' | b'"') => {
                let close = skip_string(b, i, end);
                i = close;
                (close >= at + 2 && b[close - 1] == q)
                    .then(|| String::from_utf8_lossy(&b[at + 1..close - 1]).into_owned())
            }
            c if is_ident_byte(c) && !c.is_ascii_digit() => {
                while i < end && is_ident_byte(b[i]) {
                    i += 1;
                }
                Some(String::from_utf8_lossy(&b[at..i]).into_owned())
            }
            _ => None,
        };
        let Some(key) = key else {
            // A spread or a computed key: skip the entry.
            i = value_end(b, at + 1, end);
            continue;
        };
        let j = skip_trivia(b, i, end);
        if b.get(j) != Some(&b':') || j >= end {
            out.push((Key { key, at }, j, j));
            i = value_end(b, j, end);
            continue;
        }
        let vs = skip_trivia(b, j + 1, end);
        let ve = value_end(b, vs, end);
        let mut te = ve;
        while te > vs && b[te - 1].is_ascii_whitespace() {
            te -= 1;
        }
        out.push((Key { key, at }, vs, te));
        i = ve;
    }
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// The end of the value starting at `i`: the first `,` or closing bracket at
/// depth 0, past strings, comments and nested brackets.
fn value_end(b: &[u8], mut i: usize, end: usize) -> usize {
    let mut depth = 0usize;
    while i < end {
        match b[i] {
            b'\'' | b'"' | b'`' => {
                i = skip_string(b, i, end);
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'/') || b.get(i + 1) == Some(&b'*') => {
                i = skip_trivia(b, i, end);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    return i;
                }
                depth -= 1;
            }
            b',' if depth == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    end
}

/// The content range of the value `b[vs..ve]` when it is exactly one string
/// literal (`'..'`, `".."` or a backtick literal), else `None`.
fn single_literal(b: &[u8], vs: usize, ve: usize) -> Option<(usize, usize)> {
    let q = *b.get(vs)?;
    if !matches!(q, b'\'' | b'"' | b'`') {
        return None;
    }
    let close = skip_string(b, vs, ve);
    (close >= vs + 2 && b[close - 1] == q && skip_trivia(b, close, ve) >= ve)
        .then_some((vs + 1, close - 1))
}

/// The offset just past the string literal opening at `b[i]` (escapes
/// honoured), or `end` when it never closes.
fn skip_string(b: &[u8], i: usize, end: usize) -> usize {
    let q = b[i];
    let mut j = i + 1;
    while j < end {
        if b[j] == b'\\' {
            j += 2;
            continue;
        }
        if b[j] == q {
            return j + 1;
        }
        j += 1;
    }
    end
}

/// The first offset at or after `i` that is neither whitespace nor inside a
/// `//` or `/* */` comment.
fn skip_trivia(b: &[u8], mut i: usize, end: usize) -> usize {
    while i < end {
        if b[i].is_ascii_whitespace() {
            i += 1;
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < end && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < end && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(end);
        } else {
            break;
        }
    }
    i
}

/// Given `s` starting after a `(`, return the offset of the matching `)`,
/// skipping nested parens, brackets, braces, and string literals.
fn find_balanced_paren(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1i32;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 && c == b')' {
                    return Some(i);
                }
            }
            b'\'' | b'"' | b'`' => {
                let delim = c;
                i += 1;
                while i < bytes.len() && bytes[i] != delim {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Walk forward from `s` to find the first `class <Name>` declaration.
/// Skips whitespace, line/block comments, and the `export`/`default`/
/// `abstract` modifiers commonly preceding `class` in TS.
fn find_next_class_name(s: &str) -> Option<String> {
    next_class(s).map(|(name, _)| name)
}

/// [`find_next_class_name`]'s walk, also returning the byte offset in `s`
/// just past the class name (DT.5c reads the class body from there).
fn next_class(s: &str) -> Option<(String, usize)> {
    let mut at = 0;
    for line in s.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let t = line.trim_start();
        let t_at = start + (line.len() - t.len());
        let t = t.trim_end();
        if t.is_empty()
            || t.starts_with("//")
            || t.starts_with("/*")
            || t.starts_with("*")
            || t.starts_with(')')
        {
            continue;
        }
        if let Some((name, end)) = class_name_span(t) {
            return Some((name, t_at + end));
        }
        // If the line starts with another `@<Decorator>` (additional decorators
        // are legal in TS), keep scanning rather than aborting.
        if t.starts_with('@') {
            continue;
        }
        // Otherwise we've gone too far — between this decorator and the next
        // class declaration there's other code; abort to avoid false attribution.
        return None;
    }
    None
}

/// The `<Name>` of a line opening `class <Name>`, past any `export`,
/// `default` and `abstract` modifiers (CH.1: `export abstract class X` behind
/// a decorator is a decorated class like any other).
fn extract_class_name(line: &str) -> Option<String> {
    class_name_span(line).map(|(name, _)| name)
}

/// [`extract_class_name`] with the byte offset in `line` past the name.
fn class_name_span(line: &str) -> Option<(String, usize)> {
    let trimmed = line
        .trim_start_matches("export ")
        .trim_start_matches("default ")
        .trim_start_matches("abstract ");
    let rest = trimmed.strip_prefix("class ")?;
    let rest_at = line.len() - rest.len();
    let name_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(rest.len());
    (name_end > 0).then(|| (rest[..name_end].to_string(), rest_at + name_end))
}

// ----------------------------------------------------------------------------
// DT.5c: member decorators in a class body
// ----------------------------------------------------------------------------

/// `src` with every `//` and `/* */` comment turned to spaces (newlines
/// kept, so offsets and rows stay the file's); string and template literals
/// are skipped whole, so a `//` inside a URL masks nothing.
fn mask_ts_comments(src: &[u8]) -> Vec<u8> {
    let mut b = src.to_vec();
    let n = b.len();
    let mut i = 0;
    while i < n {
        match b[i] {
            b'\'' | b'"' | b'`' => i = skip_string(&b, i, n),
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < n && b[i] != b'\n' {
                    b[i] = b' ';
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = b[i + 2..]
                    .windows(2)
                    .position(|w| w == b"*/")
                    .map_or(n, |p| i + 2 + p + 2);
                for c in &mut b[i..end] {
                    if *c != b'\n' {
                        *c = b' ';
                    }
                }
                i = end;
            }
            _ => i += 1,
        }
    }
    b
}

/// The body `(start, end)` of the class whose name ends at `b[from]`: past
/// the `{` that opens it (after `extends` / `implements` clauses, their
/// generic and call arguments skipped) up to its matching `}`. `None` when a
/// `;` or the end comes first, or the body never closes.
fn class_body(b: &[u8], from: usize) -> Option<(usize, usize)> {
    let n = b.len();
    let mut depth = 0usize;
    let mut i = from;
    let open = loop {
        let c = *b.get(i)?;
        match c {
            b'\'' | b'"' | b'`' => {
                i = skip_string(b, i, n);
                continue;
            }
            b'(' | b'<' | b'[' => depth += 1,
            // `=>` in a generic argument (`Base<() => void>`) closes nothing.
            b'>' if i > 0 && b[i - 1] == b'=' => {}
            b')' | b'>' | b']' => depth = depth.saturating_sub(1),
            b'{' if depth == 0 => break i,
            b';' if depth == 0 => return None,
            _ => {}
        }
        i += 1;
    };
    let close = bracket_close(b, open)?;
    Some((open + 1, close))
}

/// With `b[open]` an opening bracket, the offset of the bracket that closes
/// it (strings skipped, every bracket kind nested).
fn bracket_close(b: &[u8], open: usize) -> Option<usize> {
    let n = b.len();
    let mut depth = 0usize;
    let mut i = open;
    while i < n {
        match b[i] {
            b'\'' | b'"' | b'`' => {
                i = skip_string(b, i, n);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The member decorators at the top level of the class body `b[s..e]`
/// (comments already masked), each with the member it binds when that member
/// is a shape the decorator invokes ([`MemberDecorator::accepts`]).
fn member_decorators(b: &[u8], source: &str, (s, e): (usize, usize)) -> Vec<DecoratedMember> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut i = s;
    while i < e {
        match b[i] {
            b'\'' | b'"' | b'`' => {
                i = skip_string(b, i, e);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'@' if depth == 0 => {
                let Some((ns, ne)) = ident_span(b, i + 1, e) else {
                    i += 1;
                    continue;
                };
                if let Some(decorator) = MemberDecorator::named(&b[ns..ne])
                    && let Some(member_at) = decorator_end(b, ne, e)
                    && let Some((shape, name)) = member_after(b, member_at, e)
                    && decorator.accepts(shape)
                {
                    out.push(DecoratedMember {
                        decorator,
                        name,
                        row: glia_code_domain::line_of(source, i),
                    });
                }
                i = ne;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// The identifier starting at `b[i]` (not a digit), within `end`.
fn ident_span(b: &[u8], i: usize, end: usize) -> Option<(usize, usize)> {
    let c = *b.get(i).filter(|_| i < end)?;
    if !is_ident_byte(c) || c.is_ascii_digit() {
        return None;
    }
    let mut j = i;
    while j < end && is_ident_byte(b[j]) {
        j += 1;
    }
    Some((i, j))
}

/// Past a decorator's call arguments: with `b[ne]` just after `@Name`, the
/// offset after its `(..)`; `None` when no `(` follows (`@Input` written
/// without a call is no Angular decorator).
fn decorator_end(b: &[u8], ne: usize, end: usize) -> Option<usize> {
    let p = skip_ws(b, ne, end);
    if b.get(p) != Some(&b'(') || p >= end {
        return None;
    }
    let close = bracket_close(b, p).filter(|&c| c < end)?;
    Some(close + 1)
}

/// The class member a decorator ending at `b[i]` decorates: further
/// decorators (`@X(..)` / `@X`), the modifiers `public private protected
/// readonly static async override` and whitespace (blank and masked comment
/// lines included) are skipped, then `get name(` / `set name(` / `name(` (or
/// `name<T>(`). A field (`name = ..`, `name: T`, `name!: T`) is `None`.
fn member_after(b: &[u8], mut i: usize, end: usize) -> Option<(MemberShape, String)> {
    const MODIFIERS: &[&[u8]] = &[
        b"public",
        b"private",
        b"protected",
        b"readonly",
        b"static",
        b"async",
        b"override",
    ];
    let (ws, we) = loop {
        i = skip_ws(b, i, end);
        if b.get(i) == Some(&b'@') && i < end {
            let (_, ne) = ident_span(b, i + 1, end)?;
            i = decorator_end(b, ne, end).unwrap_or(ne);
            continue;
        }
        let (ws, we) = ident_span(b, i, end)?;
        let after = skip_ws(b, we, end);
        // A modifier is followed by more of the member; `async(` or
        // `static = 1` names a member of that name.
        if MODIFIERS.contains(&&b[ws..we]) && ident_span(b, after, end).is_some() {
            i = we;
            continue;
        }
        break (ws, we);
    };
    let word = &b[ws..we];
    let j = skip_ws(b, we, end);
    if matches!(word, b"get" | b"set")
        && let Some((ns, ne)) = ident_span(b, j, end)
    {
        let k = skip_ws(b, ne, end);
        if b.get(k) != Some(&b'(') || k >= end {
            return None;
        }
        let shape = if word == b"get" {
            MemberShape::Getter
        } else {
            MemberShape::Setter
        };
        return Some((shape, String::from_utf8_lossy(&b[ns..ne]).into_owned()));
    }
    matches!(b.get(j), Some(b'(' | b'<'))
        .then(|| {
            (
                MemberShape::Method,
                String::from_utf8_lossy(word).into_owned(),
            )
        })
        .filter(|_| j < end)
}

fn skip_ws(b: &[u8], mut i: usize, end: usize) -> usize {
    while i < end && i < b.len() && b[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::{CallQualifier, cell_type, edge_category};
    use glia_core::CellPayload;

    fn repo() -> RepoId {
        RepoId(1)
    }
    fn module_id() -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "test")
    }

    #[test]
    fn detects_component_service_directive_pipe() {
        let src = r#"
@Component({ selector: 'app-user' })
export class UserComponent {}

@Injectable({ providedIn: 'root' })
export class UserService {}

@Directive({ selector: '[appHi]' })
export class HiDirective {}

@Pipe({ name: 'upper' })
export class UpperPipe {}
"#;
        let r = extract_angular_nodes(src, "test", module_id(), repo());
        let get = |kind: NodeKindId| -> Vec<&str> {
            r.nav
                .kind_by_id
                .iter()
                .filter(|(_, k)| **k == kind)
                .filter_map(|(id, _)| r.nav.name_by_id.get(id).map(|s| s.as_str()))
                .collect()
        };
        assert!(get(node_kind::COMPONENT).contains(&"UserComponent"));
        assert!(get(node_kind::SERVICE).contains(&"UserService"));
        assert!(get(node_kind::DIRECTIVE).contains(&"HiDirective"));
        assert!(get(node_kind::PIPE).contains(&"UpperPipe"));
    }

    #[test]
    fn detects_multi_line_decorator_metadata() {
        // Real Angular convention: the @Component arg is a multi-line object
        // literal between the decorator and the class. Was a hard miss in the
        // 2026-05-05 framework-coverage check (angular-realworld emitted 0
        // COMPONENT despite 30+ files using this exact shape).
        let src = r#"
import { ChangeDetectionStrategy, Component } from '@angular/core';

@Component({
  selector: 'app-root',
  templateUrl: './app.component.html',
  imports: [HeaderComponent, RouterOutlet, FooterComponent],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class AppComponent {}

@Component({
  selector: 'app-profile',
  styleUrl: './profile.component.scss',
  template: `<div>profile</div>`,
})
export class ProfileComponent {}

@Injectable({
  providedIn: 'root',
})
export class JwtService {}
"#;
        let r = extract_angular_nodes(src, "test", module_id(), repo());
        let comps: Vec<&str> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::COMPONENT)
            .filter_map(|(id, _)| r.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(comps.contains(&"AppComponent"), "got components: {comps:?}");
        assert!(
            comps.contains(&"ProfileComponent"),
            "got components: {comps:?}"
        );

        let services: Vec<&str> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::SERVICE)
            .filter_map(|(id, _)| r.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(services.contains(&"JwtService"));
    }

    #[test]
    fn detects_guard() {
        let src = "export class AuthGuard { canActivate() { return true; } }";
        let r = extract_angular_nodes(src, "test", module_id(), repo());
        let guards: Vec<&str> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::GUARD)
            .filter_map(|(id, _)| r.nav.name_by_id.get(id).map(|s| s.as_str()))
            .collect();
        assert!(guards.contains(&"AuthGuard"));
    }

    #[test]
    fn detects_routes() {
        let src = r#"
const routes: Routes = [
    { path: '', component: HomeComponent },
    { path: 'users', component: UsersComponent },
    { path: 'users/:id', component: UserDetailComponent },
];
RouterModule.forRoot(routes);
"#;
        // LA.6b: Angular Router tables are the shared walker's; the Angular
        // extractor itself mints no ROUTE any more.
        let own = extract_angular_nodes(src, "test", module_id(), repo());
        assert!(own.nav.kind_by_id.values().all(|k| *k != node_kind::ROUTE));
        let r = crate::nav_routes::extract_route_tables(src, module_id(), repo());
        let names: Vec<&str> = r
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| r.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(names.contains(&"/users"));
        assert!(names.contains(&"/users/:id"));
        assert!(names.contains(&"/"));
        // LB.4c: a nav page lives in the `page:<path>` qname namespace, its
        // display name is the bare path.
        let mut qnames: Vec<&str> = r
            .nav
            .qname_by_id
            .iter()
            .filter(|(id, _)| r.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, q)| q.as_str())
            .collect();
        qnames.sort_unstable();
        assert_eq!(qnames, ["page:/", "page:/users", "page:/users/:id"]);
        assert_eq!(r.nav_routes, 3, "A3.4: every client-router ROUTE counted");
        // A3.4: each one carries the `provenance: nav_route` ORIGIN mark, which
        // is what `graph::nav::is_nav_route` reads to keep it out of
        // the HTTP pairing index.
        let route_ids: Vec<_> = r
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::ROUTE)
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(route_ids.len(), 3);
        for id in route_ids {
            let node = r.nodes.iter().find(|n| n.id == id).expect("route node");
            assert!(
                node.cells.iter().any(|c| c.kind == cell_type::ORIGIN
                    && matches!(&c.payload, CellPayload::Json(j)
                                if j.contains("\"provenance\":\"nav_route\""))),
                "nav route must carry the ORIGIN provenance mark"
            );
        }
        // LA.6b: `component: X` binds each route to its page component.
        let mut bound: Vec<(&str, &str)> = r
            .refs
            .iter()
            .filter(|x| x.category == edge_category::HANDLED_BY)
            .filter_map(|x| match &x.qualifier {
                CallQualifier::Bare(h) => {
                    Some((r.nav.qname_by_id.get(&x.from)?.as_str(), h.as_str()))
                }
                _ => None,
            })
            .collect();
        bound.sort_unstable();
        assert_eq!(
            bound,
            [
                ("page:/", "HomeComponent"),
                ("page:/users", "UsersComponent"),
                ("page:/users/:id", "UserDetailComponent"),
            ]
        );
    }

    /// DT.5a: a backtick template's raw content and first offset, the
    /// first-level `host:` entries (quoted and bare keys, quoted values only)
    /// and the class the decorator names.
    #[test]
    fn decorated_classes_read_template_and_host() {
        let src = "@Component({\n  selector: 'x', // template: 'no'\n  host: { class: 'block', '(document:keydown.escape)': \"onEscape()\", role, tabindex: 0 },\n  template: `<b (click)=\"go()\">\u{1F600}</b>`,\n  styles: [`b { template: 1 }`],\n})\nexport class AComponent {}\n";
        let got = decorated_classes(src);
        assert_eq!(got.len(), 1);
        let dc = &got[0];
        assert_eq!(dc.name.as_deref(), Some("AComponent"));
        assert_eq!(dc.kind, node_kind::COMPONENT);
        assert!(!dc.template_unread);
        let t = dc.template.as_ref().expect("template");
        assert_eq!(t.text, "<b (click)=\"go()\">\u{1F600}</b>");
        assert_eq!(&src[t.start..t.start + 2], "<b");
        assert_eq!(
            dc.host,
            vec![
                HostEntry {
                    key: "class".into(),
                    value: "block".into(),
                    row: 2
                },
                HostEntry {
                    key: "(document:keydown.escape)".into(),
                    value: "onEscape()".into(),
                    row: 2
                },
            ]
        );
    }

    /// DT.5a: a single-quote template reads; a concatenated or identifier
    /// template reads nothing and is marked unread; `templateUrl` is no
    /// template; a directive carries its host.
    #[test]
    fn decorated_classes_single_quote_concat_and_directive() {
        let src = "@Component({ template: '<i>{{ a() }}</i>' })\nclass One {}\n\
                   @Component({ template: '<i>' + x + '</i>' })\nclass Two {}\n\
                   @Component({ template: TPL })\nclass Three {}\n\
                   @Component({ templateUrl: './four.html' })\nclass Four {}\n\
                   @Directive({ selector: '[d]', host: { '[class.on]': 'isOn()' } })\nclass Dir {}\n";
        let got = decorated_classes(src);
        let names: Vec<_> = got
            .iter()
            .map(|d| d.name.clone().unwrap_or_default())
            .collect();
        assert_eq!(names, ["One", "Two", "Three", "Four", "Dir"]);
        assert_eq!(
            got[0].template.as_ref().map(|t| t.text.as_str()),
            Some("<i>{{ a() }}</i>")
        );
        for d in &got[1..3] {
            assert!(d.template.is_none() && d.template_unread, "{:?}", d.name);
        }
        assert!(got[3].template.is_none() && !got[3].template_unread);
        assert_eq!(got[3].template_url, Some(("./four.html".to_string(), 6)));
        assert!(
            got.iter()
                .filter(|d| d.name.as_deref() != Some("Four"))
                .all(|d| d.template_url.is_none())
        );
        assert_eq!(got[4].kind, node_kind::DIRECTIVE);
        assert_eq!(got[4].host.len(), 1);
        assert_eq!(got[4].host[0].row, 8);
    }

    /// DT.5b: a multi-line decorator's `templateUrl:` literal (either quote,
    /// a bare `<name>.html` with no `./`) and its 0-based row; a non-literal
    /// value, a nested object's key and a comment's text read as nothing.
    #[test]
    fn decorated_classes_read_template_url_and_row() {
        let src = "import { Component } from '@angular/core';\n\n\
                   @Component({\n  selector: 'shop-orders',\n  // templateUrl: './old.html',\n  templateUrl: './orders.page.html',\n})\nexport class OrdersPage {}\n\n\
                   @Component({ selector: 'p', templateUrl: \"user-profile.html\", styleUrl: './p.css' })\nexport class UserProfile {}\n\n\
                   @Component({ templateUrl: URL, providers: [{ templateUrl: './inner.html' }] })\nexport class Dyn {}\n";
        let got = decorated_classes(src);
        let urls: Vec<_> = got
            .iter()
            .map(|d| (d.name.clone().unwrap_or_default(), d.template_url.clone()))
            .collect();
        assert_eq!(
            urls,
            vec![
                (
                    "OrdersPage".to_string(),
                    Some(("./orders.page.html".to_string(), 5))
                ),
                (
                    "UserProfile".to_string(),
                    Some(("user-profile.html".to_string(), 9))
                ),
                ("Dyn".to_string(), None),
            ]
        );
        // A templateUrl is no inline template.
        assert!(
            got.iter()
                .all(|d| d.template.is_none() && !d.template_unread)
        );
    }

    /// CH.1: `abstract` between the decorator and `class` no longer hides the
    /// class name, so a decorated abstract class gets its overlay.
    #[test]
    fn abstract_class_after_decorator_is_named() {
        let src = "@Injectable()\nexport abstract class FilterDataService {}\n\n\
                   @Directive()\nabstract class AnalyticsDirective {}\n\n\
                   export default abstract class AuthGuard {}\n";
        assert_eq!(
            scan_decorated_classes(src),
            vec![
                ("AnalyticsDirective".to_string(), node_kind::DIRECTIVE),
                ("AuthGuard".to_string(), node_kind::GUARD),
                ("FilterDataService".to_string(), node_kind::SERVICE),
            ]
        );
    }

    fn members(src: &str) -> Vec<(MemberDecorator, String, u32)> {
        decorated_classes(src)
            .into_iter()
            .flat_map(|d| d.members)
            .map(|m| (m.decorator, m.name, m.row))
            .collect()
    }

    fn m(d: MemberDecorator, name: &str, row: u32) -> (MemberDecorator, String, u32) {
        (d, name.to_string(), row)
    }

    /// DT.5c: the member decorators of a decorated class body, each with the
    /// member Angular invokes and the decorator's 0-based row: a HostListener
    /// with arguments, a stacked decorator, a HostBinding getter (its field
    /// form binds nothing), an Input setter on its own line and on the
    /// decorator's line, a `private set` behind a ViewChild, `async` /
    /// `override` modifiers; an Input field and commented decorators read as
    /// nothing.
    #[test]
    fn decorated_classes_read_member_decorators() {
        use MemberDecorator::{HostBinding, HostListener, Input, Query};
        let src = "import { Component } from '@angular/core';\n\
                   @Component({ selector: 'x', template: '<i></i>' })\n\
                   export class SheetComponent extends Base<() => void> implements OnInit {\n\
                   \x20 open = false;\n\
                   \x20 @HostListener('window:resize', ['$event'])\n\
                   \x20 onResize(e: Event): void { const o = { a: () => {} }; }\n\
                   \x20 @HostListener('document:keydown.escape')\n\
                   \x20 @Deprecated()\n\
                   \x20 // keeps the sheet open on purpose\n\
                   \x20 public async onEscape(): Promise<void> {}\n\
                   \x20 @HostBinding('class.is-open')\n\
                   \x20 get isOpen(): boolean { return this.open; }\n\
                   \x20 @HostBinding('attr.role') role = 'dialog';\n\
                   \x20 @Input()\n\
                   \x20 set title(value: string) { this.open = value.length > 0; }\n\
                   \x20 @Input() set link(value: string) {}\n\
                   \x20 @Input() size = 1;\n\
                   \x20 @Input({ required: true }) label!: string;\n\
                   \x20 @ViewChild('qr')\n\
                   \x20 private set canvas(el: unknown) {}\n\
                   \x20 @ContentChildren(Item) override set items(q: unknown) {}\n\
                   \x20 // @HostListener('click') onClick() {}\n\
                   \x20 /* @Input() set gone(v: string) {} */\n\
                   \x20 note = '@HostListener(\"x\") fake() {}';\n\
                   \x20 close(): void { this.open = false; }\n\
                   }\n\
                   @HostListener('click') loose() {}\n";
        assert_eq!(
            members(src),
            vec![
                m(HostListener, "onResize", 4),
                m(HostListener, "onEscape", 6),
                m(HostBinding, "isOpen", 10),
                m(Input, "title", 13),
                m(Input, "link", 15),
                m(Query, "canvas", 18),
                m(Query, "items", 20),
            ]
        );
        assert_eq!(MemberDecorator::Query.rule(), "query_setter");
        assert_eq!(MemberDecorator::HostBinding.rule(), "host_binding");
    }

    /// DT.5c: the shapes each decorator accepts: a HostListener getter, an
    /// Input getter and a HostBinding setter bind nothing; a HostBinding
    /// method binds; a class with no body or a decorator with no call reads
    /// nothing.
    #[test]
    fn member_decorators_bind_only_the_invoked_shape() {
        use MemberDecorator::HostBinding;
        let src = "@Directive({ selector: '[d]' })\n\
                   export class D {\n\
                   \x20 @HostListener('click') get clicked() { return 1; }\n\
                   \x20 @Input() get value() { return 1; }\n\
                   \x20 @HostBinding('class.on') set on(v: boolean) {}\n\
                   \x20 @HostBinding('style.width') width() { return '1px'; }\n\
                   \x20 @Input set bare(v: string) {}\n\
                   }\n";
        assert_eq!(members(src), vec![m(HostBinding, "width", 5)]);
        assert!(members("@Component({})\ndeclare class X;\n").is_empty());
    }
}
