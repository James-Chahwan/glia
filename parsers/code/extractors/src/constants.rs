//! Repo-scope literal-constant table (A11.1).
//!
//! Nothing else in glia can turn an identifier into the string it holds:
//! `producer.send({topic: TOPIC})` degrades to the framework tag node and
//! `` `${environment.apiUrl}/users` `` to `/{}/users`, because there is no
//! name -> literal index anywhere. This module is that index, shared by every
//! consumer so each does not grow a private one with its own rules.
//!
//! WHICH SIDE OF THE EXTRACT/RESOLVE SPLIT. [`ConstTable::scan_file`] is
//! extraction: a pure function of one file's text, a line scanner in the
//! `config.rs` / `queues.rs` family. It resolves nothing. Merging per-file
//! tables into a repo table and looking names up in it is resolution, and it
//! happens in the ENGINE, between parse and graph build, the same seam as
//! `read_go_module_prefix` and the A5.2 proto-service set (`engine/src/build.rs`).
//! Not in the graph crate: the bindings are not nodes (the TS parser drops
//! undocumented short-string consts), and consumers need the value BEFORE node
//! identity is minted (`queue_producer:<topic>`, an ENDPOINT path).
//!
//! CACHE RULE FOR CONSUMERS. A file's parse is cached by its own content hash,
//! but a table lookup depends on OTHER files. A consumer must therefore run on
//! the router's output after the cache, like `apply_rpc_needles`, never
//! inside the per-file cross-cutting extractors, or an incremental build replays
//! folds made against a stale table.
//!
//! ENV VALUES are out of scope by construction: `.env` / yaml / Dockerfile never
//! reach [`ConstTable::scan_file`] (the engine only scans files with a source
//! language), and [`ConstTable::resolve_expr`] refuses `process.env.X`-style
//! reads. Define-side env values live on A13.7's redacted `cell_type::ENV` cell.
//!
//! No panics: every slice goes through `get()` or lands on an ASCII byte.

use std::collections::BTreeMap;

/// Longest literal kept. Anything longer is a payload, not a name or a URL.
pub const MAX_VALUE_LEN: usize = 512;

/// Longest binding name (one dotted segment) accepted.
pub const MAX_NAME_LEN: usize = 128;

/// What [`fold_interpolations`] writes for a `${…}` span it could not resolve.
/// It still contains `${`, so the HTTP path normaliser keeps folding it to `{}`.
pub const UNRESOLVED_SPAN: &str = "${…}";

/// Bracket depth past which the object-literal walk gives up on a file.
const MAX_OBJECT_DEPTH: usize = 32;

/// Name -> literal bindings for one file or one repo. `BTreeMap`, and first
/// binding wins over the engine's sorted file list, so the table is identical
/// across processes (the reason `walk_source_files` sorts).
#[derive(Debug, Default, Clone)]
pub struct ConstTable {
    by_key: BTreeMap<String, String>,
    /// Extra distinct values seen for a key after its first binding, in the
    /// order they were seen. A key present here is ambiguous.
    alternatives: BTreeMap<String, Vec<String>>,
    files: usize,
}

impl ConstTable {
    /// Every literal binding `source` declares. Line-oriented; `lang` is the
    /// engine's language tag and only selects which declaration shapes apply.
    pub fn scan_file(source: &str, lang: &str) -> Self {
        let ts = matches!(lang, "typescript" | "react" | "angular" | "vue");
        let mut t = ConstTable::default();
        let mut chain: Vec<Option<String>> = Vec::new();
        let mut go_block = false;
        for line in source.lines() {
            if !chain.is_empty() {
                scan_object(line, &mut chain, &mut t);
                continue;
            }
            let body = line.trim();
            if lang == "go" {
                if go_block {
                    go_block = !body.starts_with(')');
                    if let Some((name, value)) = go_block_line(body) {
                        t.insert(name, value);
                    }
                    continue;
                }
                if go_block_opener(body) {
                    go_block = true;
                    continue;
                }
            }
            if let Some((name, value)) = binding(line, body, lang) {
                t.insert(name, value);
            } else if ts && let Some((name, rest)) = object_opener(body) {
                chain.push(Some(name.to_string()));
                scan_object(rest, &mut chain, &mut t);
            }
        }
        t.files = usize::from(!t.is_empty());
        t
    }

    /// Fold `other` in. The existing binding wins; a differing value is kept
    /// as an alternative and counted as a conflict.
    pub fn merge_from(&mut self, other: &ConstTable) {
        self.files += other.files;
        for (k, v) in &other.by_key {
            self.bind(k, v);
        }
        for (k, alts) in &other.alternatives {
            for v in alts {
                self.bind(k, v);
            }
        }
    }

    /// The first value bound to exactly `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.by_key.get(key).map(String::as_str)
    }

    /// Resolve a source expression, leniently: normalise it (drop `this.` /
    /// `self.` / `$` / `@` / a trailing `()`, read `::` / `->` / `?.` as `.`),
    /// then try the full dotted key, then its last segment. First binding wins
    /// even for an ambiguous key. Right for a base URL, where a guess only
    /// sharpens a match the resolver would otherwise make at lower confidence.
    pub fn resolve_expr(&self, expr: &str) -> Option<&str> {
        let key = normalise_expr(expr)?;
        self.get(&key).or_else(|| self.get(key.rsplit('.').next()?))
    }

    /// Resolve for a consumer that MINTS IDENTITY from the value (a queue
    /// topic): the exact normalised key only, and never an ambiguous one. A
    /// wrong topic manufactures a false cross-service edge; no topic does not.
    pub fn resolve_expr_strict(&self, expr: &str) -> Option<&str> {
        let key = normalise_expr(expr)?;
        if self.alternatives.contains_key(&key) {
            return None;
        }
        self.get(&key)
    }

    /// Every distinct value bound to the exact normalised key, first binding
    /// first. `environment.apiUrl` is routinely bound once per deployment file,
    /// so a host-based consumer must treat it as a set.
    pub fn candidates(&self, expr: &str) -> Vec<&str> {
        let Some(key) = normalise_expr(expr) else {
            return Vec::new();
        };
        let first = self.get(&key).into_iter();
        let rest = self.alternatives.get(&key).into_iter().flatten();
        first.chain(rest.map(String::as_str)).collect()
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Files that contributed at least one binding.
    pub fn files(&self) -> usize {
        self.files
    }

    /// Distinct extra values seen for already-bound keys.
    pub fn conflicts(&self) -> usize {
        self.alternatives.values().map(Vec::len).sum()
    }

    /// Validate, redact, then bind.
    fn insert(&mut self, name: &str, value: &str) {
        if is_secret_name(name) {
            return;
        }
        if let Some(v) = accept_value(value) {
            self.bind(name, &v);
        }
    }

    fn bind(&mut self, key: &str, value: &str) {
        match self.by_key.get(key) {
            None => {
                self.by_key.insert(key.to_string(), value.to_string());
            }
            Some(first) if first == value => {}
            Some(_) => {
                let alts = self.alternatives.entry(key.to_string()).or_default();
                if !alts.iter().any(|a| a == value) {
                    alts.push(value.to_string());
                }
            }
        }
    }
}

/// Substitute every `${expr}` in `raw` the table can resolve. `Some` only if at
/// least one span resolved; unresolved spans become [`UNRESOLVED_SPAN`].
///
/// The LEADING span is a base URL and resolves leniently (the same leading-slot
/// inference as the HTTP resolver's BaseFold tier). A later span is usually a
/// path parameter, so it resolves strictly and only when it is constant-shaped
/// (dotted or ALL_CAPS): `/users/${id}` must stay a parameter even when some
/// file happens to bind `id`.
pub fn fold_interpolations(raw: &str, table: &ConstTable) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    let mut hit = false;
    while let Some(start) = rest.find("${") {
        let leading = out.is_empty() && start == 0;
        out.push_str(rest.get(..start)?);
        let inner = rest.get(start + 2..)?;
        let Some(end) = closing_brace(inner) else {
            out.push_str(rest.get(start..)?);
            rest = "";
            break;
        };
        let expr = inner.get(..end)?;
        let value = if leading {
            table.resolve_expr(expr)
        } else if constant_shaped(expr) {
            table.resolve_expr_strict(expr)
        } else {
            None
        };
        match value {
            Some(v) => {
                out.push_str(v);
                hit = true;
            }
            None => out.push_str(UNRESOLVED_SPAN),
        }
        rest = inner.get(end + 1..)?;
    }
    out.push_str(rest);
    hit.then_some(out)
}

// ---------------------------------------------------------------------------
// Declaration shapes
// ---------------------------------------------------------------------------

/// Words that make a head a declaration in the keyword languages.
const DECL_KEYWORDS: &[&str] = &[
    "const",
    "let",
    "var",
    "val",
    "static",
    "final",
    "readonly",
    "export",
    "pub",
    "pub(crate)",
    "pub(super)",
    "public",
    "private",
    "protected",
    "internal",
];

/// Words that mean the `=` belongs to a statement, not a declaration.
const CONTROL_WORDS: &[&str] = &[
    "return", "if", "else", "elif", "for", "while", "case", "when", "yield", "await", "throw",
    "not", "and", "or", "in", "is", "echo", "print", "new", "typeof", "delete", "type", "unless",
    "until", "do", "then", "with", "assert", "del", "raise", "from", "import", "using", "alias",
];

/// Elixir attributes that are documentation or typespecs, never constants.
const ELIXIR_RESERVED: &[&str] = &[
    "doc",
    "moduledoc",
    "typedoc",
    "spec",
    "type",
    "typep",
    "opaque",
    "callback",
    "impl",
    "behaviour",
    "derive",
    "deprecated",
    "since",
    "external_resource",
    "compile",
    "dialyzer",
];

/// One `name -> literal` binding on `line` (`body` is it trimmed).
fn binding<'a>(line: &str, body: &'a str, lang: &str) -> Option<(&'a str, &'a str)> {
    match lang {
        // Module constants: column 0 and SCREAMING_CASE only, so function
        // locals and shell-ish assignments never bind.
        "python" => {
            if line.starts_with([' ', '\t']) {
                return None;
            }
            let (head, rhs) = split_assign(body)?;
            let name = before_annotation(head).trim();
            screaming(name).then_some(())?;
            Some((name, literal_rhs(rhs)?))
        }
        // A capitalised Ruby assignment is always a constant, at any indent.
        "ruby" => {
            let (head, rhs) = split_assign(body)?;
            let name = head.trim();
            screaming(name).then_some(())?;
            Some((name, literal_rhs(rhs)?))
        }
        "elixir" => {
            let rest = body.strip_prefix('@')?;
            let (name, rhs) = rest.split_at(ident_end(rest));
            if ELIXIR_RESERVED.contains(&name) || !rhs.starts_with([' ', '\t']) {
                return None;
            }
            Some((name, literal_rhs(rhs)?))
        }
        "php" => define_call(body).or_else(|| decl(body, lang)),
        "c_cpp" => hash_define(body).or_else(|| decl(body, lang)),
        "go" => short_decl(body).or_else(|| decl(body, lang)),
        "terraform" | "clojure" | "proto" => None,
        _ => decl(body, lang),
    }
}

/// `[modifiers] [type] NAME[: T] = "lit"` — TS/JS, Rust, Java/Kotlin, C#,
/// Swift, Scala, Dart, Solidity, C/C++, PHP class consts, Go `const`/`var`.
fn decl<'a>(body: &'a str, lang: &str) -> Option<(&'a str, &'a str)> {
    let (head, rhs) = split_assign(body)?;
    Some((decl_name(head, lang)?, literal_rhs(rhs)?))
}

fn decl_name<'a>(head: &'a str, lang: &str) -> Option<&'a str> {
    let toks: Vec<&str> = before_annotation(head).split_whitespace().collect();
    let (name, prefix) = if lang == "go" {
        // Go puts the type AFTER the name: `const Name string = "x"`.
        match toks.as_slice() {
            [kw, name] | [kw, name, _] if matches!(*kw, "const" | "var") => (*name, &toks[..1]),
            _ => return None,
        }
    } else {
        let (name, prefix) = toks.split_last()?;
        (*name, prefix)
    };
    // A bare `*` (a JSDoc line) or `//` is not a type.
    let typeish = |t: &&str| {
        matches!(*t, "pub(crate)" | "pub(super)")
            || (t.chars().any(|c| c.is_ascii_alphanumeric())
                && t.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_.<>,?*&[]:".contains(c)))
    };
    if prefix.is_empty()
        || prefix
            .iter()
            .any(|t| CONTROL_WORDS.contains(t) || !typeish(t))
    {
        return None;
    }
    let needs_keyword = matches!(
        lang,
        "typescript" | "react" | "angular" | "vue" | "php" | "swift" | "rust" | "scala"
    );
    if needs_keyword && !prefix.iter().any(|t| DECL_KEYWORDS.contains(t)) {
        return None;
    }
    let name = name.trim_start_matches(['*', '&']);
    let name = name.strip_suffix("[]").unwrap_or(name);
    valid_name(name).then_some(name)
}

/// Go `Name := "lit"`.
fn short_decl(body: &str) -> Option<(&str, &str)> {
    let (head, rhs) = body.split_once(":=")?;
    let name = head.trim();
    valid_name(name).then_some(())?;
    Some((name, literal_rhs(rhs)?))
}

/// Go `const (` / `var (`, optionally followed by a comment.
fn go_block_opener(body: &str) -> bool {
    let Some(rest) = body
        .strip_prefix("const")
        .or_else(|| body.strip_prefix("var"))
    else {
        return false;
    };
    let Some(rest) = rest.trim_start().strip_prefix('(') else {
        return false;
    };
    let rest = rest.trim_start();
    rest.is_empty() || rest.starts_with("//")
}

/// A line inside Go `const (` / `var (`: `Name [Type] = "lit"`.
fn go_block_line(body: &str) -> Option<(&str, &str)> {
    let (head, rhs) = split_assign(body)?;
    let toks: Vec<&str> = head.split_whitespace().collect();
    let name = match toks.as_slice() {
        [name] | [name, _] => *name,
        _ => return None,
    };
    valid_name(name).then_some(())?;
    Some((name, literal_rhs(rhs)?))
}

/// PHP `define('NAME', 'lit');`.
fn define_call(body: &str) -> Option<(&str, &str)> {
    let rest = body
        .strip_prefix("define")?
        .trim_start()
        .strip_prefix('(')?;
    let (name, rest) = quoted(rest.trim_start())?;
    let rest = rest.trim_start().strip_prefix(',')?;
    let (value, rest) = quoted(rest.trim_start())?;
    let rest = rest.trim_start().strip_prefix(')')?;
    (valid_name(name) && terminal(rest)).then_some((name, value))
}

/// C/C++ `#define NAME "lit"`.
fn hash_define(body: &str) -> Option<(&str, &str)> {
    let rest = body
        .strip_prefix('#')?
        .trim_start()
        .strip_prefix("define")?;
    if !rest.starts_with([' ', '\t']) {
        return None;
    }
    let rest = rest.trim_start();
    let (name, rhs) = rest.split_at(ident_end(rest));
    if !valid_name(name) || !rhs.starts_with([' ', '\t']) {
        return None;
    }
    Some((name, literal_rhs(rhs)?))
}

/// TS `[export] const OBJ[: T] = {` — the rest of the line after the `{`.
fn object_opener(body: &str) -> Option<(&str, &str)> {
    let (head, rhs) = split_assign(body)?;
    let rest = rhs.trim_start().strip_prefix('{')?;
    Some((decl_name(head, "typescript")?, rest))
}

/// One line of a TS object literal. `chain` holds the open brackets:
/// `Some(key)` for a named object, `None` for an array, call or function
/// body. A member binds only while every open bracket is a named object, as
/// both `OBJ.path.key` and bare `key`.
fn scan_object(line: &str, chain: &mut Vec<Option<String>>, t: &mut ConstTable) {
    let b = line.as_bytes();
    let mut i = 0;
    let mut member_start = true;
    while !chain.is_empty() {
        let Some(&c) = b.get(i) else { break };
        match c {
            b' ' | b'\t' | b'\r' => {
                i += 1;
                continue;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => break,
            b',' => {
                member_start = true;
                i += 1;
                continue;
            }
            b'{' | b'[' | b'(' => {
                chain.push(None);
                if chain.len() > MAX_OBJECT_DEPTH {
                    chain.clear();
                }
                member_start = true;
                i += 1;
                continue;
            }
            b'}' | b']' | b')' => {
                chain.pop();
                member_start = false;
                i += 1;
                continue;
            }
            _ => {}
        }
        let tail = line.get(i..).unwrap_or("");
        if member_start && let Some((key, after)) = member_key(tail) {
            let after = after.trim_start();
            if let Some(rest) = after.strip_prefix('{') {
                chain.push(Some(key.to_string()));
                i = line.len() - rest.len();
                continue;
            }
            if let Some((value, rest)) = quoted(after) {
                let r = rest.trim_start();
                let ends = r.is_empty() || r.starts_with([',', '}']) || r.starts_with("//");
                if ends && chain.iter().all(Option::is_some) {
                    let path: Vec<&str> = chain.iter().flatten().map(String::as_str).collect();
                    t.insert(&format!("{}.{key}", path.join(".")), value);
                    t.insert(key, value);
                }
                i = line.len() - rest.len();
            } else {
                i = line.len() - after.len();
            }
            member_start = false;
            continue;
        }
        member_start = false;
        if matches!(c, b'"' | b'\'' | b'`') {
            // A string that does not close on this line (a multi-line
            // template) ends the walk of the line.
            match skip_string(tail) {
                Some(rest) => i = line.len() - rest.len(),
                None => break,
            }
        } else {
            i += 1;
        }
    }
}

/// `key:` or `'key':` at the start of `s`; returns the key and what follows
/// the colon.
fn member_key(s: &str) -> Option<(&str, &str)> {
    let (key, rest) = match s.as_bytes().first()? {
        b'"' | b'\'' => quoted(s)?,
        _ => s.split_at(ident_end(s)),
    };
    let rest = rest.trim_start().strip_prefix(':')?;
    (valid_name(key) && !rest.starts_with(':')).then_some((key, rest))
}

// ---------------------------------------------------------------------------
// Lexical helpers
// ---------------------------------------------------------------------------

/// Split on the first `=` when it is a plain assignment (not `==`, `=>`,
/// `!=`, `+=`, `:=`, ...). Only the first `=` is considered.
fn split_assign(body: &str) -> Option<(&str, &str)> {
    let i = body.find('=')?;
    let b = body.as_bytes();
    let prev = i.checked_sub(1).and_then(|p| b.get(p)).copied();
    let next = b.get(i + 1).copied();
    let compound = matches!(
        prev,
        Some(b'=' | b'!' | b'<' | b'>' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^')
            | Some(b':' | b'?' | b'.')
    );
    if compound || matches!(next, Some(b'=' | b'>' | b'~')) {
        return None;
    }
    Some((body.get(..i)?, body.get(i + 1..)?))
}

/// `head` up to a lone `:` type annotation (`::` paths are kept).
fn before_annotation(head: &str) -> &str {
    let b = head.as_bytes();
    let lone = (0..b.len()).find(|&i| {
        b[i] == b':' && b.get(i + 1) != Some(&b':') && (i == 0 || b.get(i - 1) != Some(&b':'))
    });
    lone.and_then(|i| head.get(..i)).unwrap_or(head)
}

/// The right-hand side is exactly one string literal.
fn literal_rhs(rhs: &str) -> Option<&str> {
    let (value, rest) = quoted(rhs.trim_start())?;
    terminal(rest).then_some(value)
}

/// What may follow a literal to end a declaration: TS `as const`, Ruby
/// `.freeze`, a `;`, and a trailing comment.
fn terminal(rest: &str) -> bool {
    let r = rest.trim_start();
    let r = r.strip_prefix("as const").unwrap_or(r);
    let r = r.strip_prefix(".freeze").unwrap_or(r);
    let r = r.trim_start();
    let r = r.strip_prefix(';').unwrap_or(r).trim();
    r.is_empty() || r.starts_with("//") || r.starts_with('#') || r.starts_with("/*")
}

/// A `"…"`, `'…'` or `` `…` `` literal at the start of `s` with no escapes:
/// its body and what follows the closing quote.
fn quoted(s: &str) -> Option<(&str, &str)> {
    let q = *s.as_bytes().first()?;
    if !matches!(q, b'"' | b'\'' | b'`') {
        return None;
    }
    let body = s.get(1..)?;
    let end = body.find(char::from(q))?;
    let value = body.get(..end)?;
    if value.contains('\\') {
        return None;
    }
    Some((value, body.get(end + 1..)?))
}

/// Skip a string literal (escapes honoured); `None` if it does not close.
fn skip_string(s: &str) -> Option<&str> {
    let mut chars = s.char_indices();
    let (_, q) = chars.next()?;
    while let Some((i, c)) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c == q {
            return s.get(i + c.len_utf8()..);
        }
    }
    None
}

/// A literal worth binding, redacted the way A13.7 redacts an ENV value.
fn accept_value(v: &str) -> Option<String> {
    let interpolated = v.contains("${")
        || v.contains("#{")
        || v.as_bytes().windows(2).any(|w| {
            w[0] == b'$' && (w[1].is_ascii_alphabetic() || matches!(w[1], b'_' | b'{' | b'('))
        });
    if v.is_empty() || v.len() > MAX_VALUE_LEN || v.contains(['\n', '\r']) || interpolated {
        return None;
    }
    Some(mask_userinfo(v).unwrap_or_else(|| v.to_string()))
}

fn ident_end(s: &str) -> usize {
    s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len())
}

/// `[A-Za-z_][A-Za-z0-9_]*`, at most [`MAX_NAME_LEN`] bytes.
fn valid_name(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= MAX_NAME_LEN
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `^[A-Z][A-Z0-9_]*$`.
fn screaming(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_uppercase())
        && valid_name(s)
        && !s.chars().any(|c| c.is_ascii_lowercase())
}

/// Dotted, or a single SCREAMING_CASE name — a constant by convention, not a
/// local.
fn constant_shaped(expr: &str) -> bool {
    normalise_expr(expr).is_some_and(|k| k.contains('.') || screaming(&k))
}

/// The lookup key for a source expression, or `None` if it is not a plain
/// identifier chain or reads the environment.
fn normalise_expr(expr: &str) -> Option<String> {
    let s = expr
        .trim()
        .replace("::", ".")
        .replace("->", ".")
        .replace("?.", ".")
        .replace("!.", ".");
    let s = s.trim_start_matches(['$', '@']);
    let s = s.strip_suffix("()").unwrap_or(s);
    let s = ["this.", "self.", "static."]
        .iter()
        .find_map(|p| s.strip_prefix(p))
        .unwrap_or(s);
    let segs: Vec<&str> = s.split('.').collect();
    let (_, owners) = segs.split_last()?;
    let env = owners
        .iter()
        .any(|o| o.eq_ignore_ascii_case("env") || o.eq_ignore_ascii_case("environ"));
    (!env && segs.iter().all(|seg| valid_name(seg))).then(|| segs.join("."))
}

// ---------------------------------------------------------------------------
// Redaction — STOPGAP copies of A13.7's private helpers in `config.rs`.
// REMOVAL: make `config::is_secret_name` and `config::mask_userinfo`
// `pub(crate)`, call them here, and delete these two copies.
// ---------------------------------------------------------------------------

const SECRET_NEEDLES: [&str; 12] = [
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "TOKEN",
    "APIKEY",
    "API_KEY",
    "PRIVATE_KEY",
    "CREDENTIAL",
    "ACCESS_KEY",
    "SESSION_KEY",
    "SALT",
    "SIGNING",
];

/// A binding whose name looks secret is never held: its value would reach a
/// qname or cell the moment a consumer folds it.
fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_NEEDLES.iter().any(|needle| upper.contains(needle))
}

/// `scheme://user:pass@host/path` -> `scheme://***@host/path`; the host is kept.
fn mask_userinfo(value: &str) -> Option<String> {
    let scheme_end = value.find("://")? + 3;
    let rest = value.get(scheme_end..)?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let at = rest.get(..authority_end)?.rfind('@')?;
    if !rest.get(..at)?.contains(':') {
        return None;
    }
    Some(format!(
        "{}***{}",
        value.get(..scheme_end)?,
        rest.get(at..)?
    ))
}

/// Innermost-aware `}` matching the `${` that `s` follows.
fn closing_brace(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(src: &str, lang: &str) -> ConstTable {
        ConstTable::scan_file(src, lang)
    }

    #[test]
    fn ts_const_let_var_and_class_fields_bind() {
        let t = scan(
            "export const TOPIC = \"orders.created\";\n\
             const API: string = 'http://api:8080' as const;\n\
             let local = `plain`;\n\
             /**\n * const DOC = 'jsdoc example';\n */\n\
             class S {\n  private readonly baseUrl = 'http://users:8080';\n  bare = 'no';\n}\n",
            "typescript",
        );
        assert_eq!(t.get("DOC"), None, "a JSDoc `*` is not a type");
        assert_eq!(t.get("TOPIC"), Some("orders.created"));
        assert_eq!(t.get("API"), Some("http://api:8080"));
        assert_eq!(t.get("local"), Some("plain"));
        assert_eq!(t.get("baseUrl"), Some("http://users:8080"));
        assert_eq!(t.get("bare"), None, "no declaring keyword");
        assert_eq!(t.files(), 1);
    }

    #[test]
    fn python_module_constants_are_column_zero_screaming_only() {
        let t = scan(
            "TOPIC = 'orders'  # the topic\nAPI_URL: str = \"http://x\"\n\
             lower = 'nope'\nclass C:\n    INNER = 'nope'\n",
            "python",
        );
        assert_eq!(t.get("TOPIC"), Some("orders"));
        assert_eq!(t.get("API_URL"), Some("http://x"));
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn go_short_decl_typed_const_and_const_block() {
        let t = scan(
            "const Typed string = \"typed\"\nvar Plain = \"plain\"\n\
             const (\n\tOrders = \"orders\"\n\tPayments string = \"payments\" // c\n\tN = iota\n)\n\
             func f() {\n\ttopic := \"short\"\n\tif x == \"y\" {}\n}\n",
            "go",
        );
        assert_eq!(t.get("Typed"), Some("typed"));
        assert_eq!(t.get("Plain"), Some("plain"));
        assert_eq!(t.get("Orders"), Some("orders"));
        assert_eq!(t.get("Payments"), Some("payments"));
        assert_eq!(t.get("topic"), Some("short"));
        assert_eq!(t.len(), 5);
    }

    #[test]
    fn rust_const_and_static_str() {
        let t = scan(
            "pub const TOPIC: &str = \"orders\";\npub(crate) static BASE: &'static str = \"/api\";\n\
             let x = \"local\";\nfoo = \"not a decl\";\n",
            "rust",
        );
        assert_eq!(t.get("TOPIC"), Some("orders"));
        assert_eq!(t.get("BASE"), Some("/api"));
        assert_eq!(t.get("x"), Some("local"));
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn java_csharp_dart_cpp_typed_declarations() {
        let java = scan(
            "public static final String TOPIC = \"orders\";\nreturn topic = \"no\";\nconst val KT = \"kotlin\"\n",
            "java",
        );
        assert_eq!(java.get("TOPIC"), Some("orders"));
        assert_eq!(java.get("KT"), Some("kotlin"));
        assert_eq!(java.len(), 2, "a control word is not a type");
        let cs = scan("public const string Topic = \"orders\";\n", "csharp");
        assert_eq!(cs.get("Topic"), Some("orders"));
        let dart = scan("static const String base = 'http://d';\n", "dart");
        assert_eq!(dart.get("base"), Some("http://d"));
        let cpp = scan(
            "#define TOPIC \"orders\"\nstatic const std::string HOST = \"h\";\nconst char *P = \"p\";\n",
            "c_cpp",
        );
        assert_eq!(cpp.get("TOPIC"), Some("orders"));
        assert_eq!(cpp.get("HOST"), Some("h"));
        assert_eq!(cpp.get("P"), Some("p"));
    }

    #[test]
    fn php_define_and_class_const() {
        let t = scan(
            "<?php\ndefine('TOPIC', 'orders');\nclass C { }\n    public const BASE = \"/api\";\n$x = 'no';\n",
            "php",
        );
        assert_eq!(t.get("TOPIC"), Some("orders"));
        assert_eq!(t.get("BASE"), Some("/api"));
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn ruby_constants_and_elixir_attributes() {
        let rb = scan(
            "module Topics\n  ORDERS = 'orders'.freeze\n  local = 'no'\nend\n",
            "ruby",
        );
        assert_eq!(rb.get("ORDERS"), Some("orders"));
        assert_eq!(rb.len(), 1);
        let ex = scan(
            "defmodule M do\n  @topic \"orders\"\n  @doc \"Documentation\"\n  @moduledoc false\nend\n",
            "elixir",
        );
        assert_eq!(ex.get("topic"), Some("orders"));
        assert_eq!(ex.len(), 1);
    }

    #[test]
    fn object_literal_binds_dotted_and_bare_keys() {
        let t = scan(
            "export const environment = {\n  production: false,\n  apiUrl: 'http://users-service:8080',\n\
             \x20 'quoted': \"q\",\n  api: {\n    base: '/v1', // c\n  },\n  list: [{ inner: 'no' }],\n\
             \x20 computed: base + '/x',\n  fn() { return { deep: 'no' }; },\n};\n\
             const AFTER = 'after';\n",
            "typescript",
        );
        assert_eq!(
            t.get("environment.apiUrl"),
            Some("http://users-service:8080")
        );
        assert_eq!(t.get("apiUrl"), Some("http://users-service:8080"));
        assert_eq!(t.get("environment.quoted"), Some("q"));
        assert_eq!(t.get("environment.api.base"), Some("/v1"));
        assert_eq!(t.get("base"), Some("/v1"));
        assert_eq!(t.get("inner"), None, "array element is not a named member");
        assert_eq!(t.get("computed"), None);
        assert_eq!(t.get("deep"), None, "function body is not a named member");
        assert_eq!(
            t.get("AFTER"),
            Some("after"),
            "the walk closed at the final brace"
        );
    }

    #[test]
    fn single_line_object_literal() {
        let t = scan(
            "const Topics = { ORDERS: 'orders', PAY: 'pay' } as const;\n",
            "typescript",
        );
        assert_eq!(t.get("Topics.ORDERS"), Some("orders"));
        assert_eq!(t.get("Topics.PAY"), Some("pay"));
    }

    #[test]
    fn resolve_expr_normalises_every_spelling() {
        let t = scan(
            "export const environment = {\n  apiUrl: 'http://api',\n};\nexport const TOPIC = 'orders';\n",
            "typescript",
        );
        for expr in [
            "environment.apiUrl",
            " this.apiUrl ",
            "self.apiUrl",
            "$this->apiUrl",
            "Config::apiUrl",
            "environment?.apiUrl",
            "other.apiUrl",
        ] {
            assert_eq!(t.resolve_expr(expr), Some("http://api"), "{expr}");
        }
        assert_eq!(t.resolve_expr("TOPIC"), Some("orders"));
        assert_eq!(t.resolve_expr("@TOPIC"), Some("orders"));
        assert_eq!(t.resolve_expr("TOPIC()"), Some("orders"));
        assert_eq!(t.resolve_expr("missing"), None);
        assert_eq!(t.resolve_expr("a + b"), None);
    }

    #[test]
    fn env_reads_never_resolve_through_the_table() {
        let t = scan("export const API_URL = 'http://source';\n", "typescript");
        assert_eq!(t.resolve_expr("API_URL"), Some("http://source"));
        assert_eq!(t.resolve_expr("process.env.API_URL"), None);
        assert_eq!(t.resolve_expr("import.meta.env.API_URL"), None);
        assert_eq!(t.resolve_expr("os.getenv('API_URL')"), None);
        assert_eq!(scan("API_URL=http://dotenv\n", "python").len(), 0);
    }

    #[test]
    fn merge_from_is_first_wins_and_counts_conflicts() {
        let mut repo = ConstTable::default();
        repo.merge_from(&scan(
            "export const TOPIC = 'a';\nexport const ONLY = 'x';\n",
            "typescript",
        ));
        repo.merge_from(&scan("export const TOPIC = 'b';\n", "typescript"));
        repo.merge_from(&scan("export const TOPIC = 'a';\n", "typescript"));
        repo.merge_from(&ConstTable::default());
        assert_eq!(repo.get("TOPIC"), Some("a"));
        assert_eq!(repo.conflicts(), 1);
        assert_eq!(repo.files(), 3);
        assert_eq!(repo.len(), 2);
        assert_eq!(repo.resolve_expr("TOPIC"), Some("a"), "lenient keeps first");
        assert_eq!(
            repo.resolve_expr_strict("TOPIC"),
            None,
            "strict refuses ambiguity"
        );
        assert_eq!(repo.resolve_expr_strict("ONLY"), Some("x"));
        assert_eq!(
            repo.resolve_expr_strict("x.ONLY"),
            None,
            "strict never falls back"
        );
        assert_eq!(repo.candidates("TOPIC"), vec!["a", "b"]);
    }

    #[test]
    fn fold_interpolations_is_none_when_nothing_resolves() {
        let t = scan("export const TOPIC = 'orders';\n", "typescript");
        assert_eq!(
            fold_interpolations("${environment.apiUrl}/users/${id}", &t),
            None
        );
        assert_eq!(fold_interpolations("/users", &t), None);
        assert_eq!(fold_interpolations("${unterminated", &t), None);
    }

    #[test]
    fn fold_interpolations_rewrites_unresolved_spans() {
        let t = scan(
            "export const environment = {\n  apiUrl: 'http://users-service:8080',\n};\n\
             export const VERSION = 'v2';\nexport const id = 'fixture-id';\n",
            "typescript",
        );
        assert_eq!(
            fold_interpolations("${environment.apiUrl}/users/${id}", &t).as_deref(),
            Some("http://users-service:8080/users/${…}"),
            "a bare lower-case later span is a parameter even when bound"
        );
        assert_eq!(
            fold_interpolations("${ this.apiUrl }/${VERSION}/x/${a.b}", &t).as_deref(),
            Some("http://users-service:8080/v2/x/${…}")
        );
        assert_eq!(
            fold_interpolations("/api/${VERSION}/users", &t).as_deref(),
            Some("/api/v2/users")
        );
    }

    #[test]
    fn oversized_empty_and_non_literal_values_are_rejected() {
        let long = "x".repeat(600);
        let src = format!(
            "export const BIG = '{long}';\nexport const EMPTY = '';\n\
             export const CAT = 'http://' + host;\nexport const TPL = `${{base}}/x`;\n\
             export const ESC = \"a\\\"b\";\nexport type Kind = 'a' | 'b';\n\
             if (x === 'y') {{}}\nexport const {} = 'long name';\n",
            "N".repeat(MAX_NAME_LEN + 1)
        );
        let t = scan(&src, "typescript");
        assert!(t.is_empty(), "{t:?}");
        assert_eq!(t.files(), 0);
        let kt = scan("val url = \"$host/api\"\n", "java");
        assert!(kt.is_empty(), "a Kotlin template is not a literal");
    }

    #[test]
    fn secret_named_bindings_are_withheld_and_userinfo_masked() {
        let t = scan(
            "export const API_KEY = 'sk_live_abc';\nexport const JWT_SECRET = 's';\n\
             export const DB_URL = 'postgres://app:hunter2@db:5432/app';\n\
             export const cfg = {\n  auth: { token: 'tok' },\n  url: 'http://ok',\n};\n",
            "typescript",
        );
        assert_eq!(t.get("API_KEY"), None);
        assert_eq!(t.get("JWT_SECRET"), None);
        assert_eq!(t.get("DB_URL"), Some("postgres://***@db:5432/app"));
        assert_eq!(t.get("cfg.auth.token"), None);
        assert_eq!(t.get("token"), None);
        assert_eq!(t.get("cfg.url"), Some("http://ok"));
        assert!(!format!("{t:?}").contains("hunter2"));
        assert!(!format!("{t:?}").contains("sk_live"));
    }

    #[test]
    fn hostile_input_never_panics() {
        let src = "const é = 'ü';\nexport const x = {\n  é: 'x', 'ünï': \"ok\", `open\n\
                   \u{1F600}: 'emoji',\n}\nconst y = { a: '\u{1F600}' };\n=\n'\n{{{{\n";
        for lang in [
            "typescript",
            "go",
            "python",
            "ruby",
            "elixir",
            "php",
            "c_cpp",
            "java",
            "clojure",
        ] {
            let _ = scan(src, lang);
        }
        let t = scan(src, "typescript");
        assert_eq!(t.get("y.a"), Some("\u{1F600}"));
        let deep = format!("const d = {}\n", "{".repeat(100));
        let d = scan(&format!("{deep}const AFTER = 'a';\n"), "typescript");
        assert_eq!(d.get("AFTER"), Some("a"), "depth bail resets the walk");
        let empty = ConstTable::default();
        assert_eq!(fold_interpolations("${}${", &empty), None);
    }
}
