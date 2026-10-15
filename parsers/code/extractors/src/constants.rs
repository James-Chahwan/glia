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
//! PINS (LF.2d). A repo's `.glia/overlay.toml` `[constants]` binds a name the
//! source cannot (`const GATEWAY = process.env.GATEWAY_URL`) through
//! [`ConstTable::pin`]: the same secret-name and value gates as a scanned
//! binding, then it REPLACES the key's binding and clears its alternatives, so
//! first-wins cannot override it and the strict lookups see one value. Pins
//! are applied after every source merge.
//!
//! CONFIG EXPRESSIONS (DF.20). A TS `const|let|var NAME = <config
//! expression>` on one line (`export const ADMIN_BASE =
//! environment.apiUrl.replace('/api', '') + '/admin'`) binds NAME to the
//! expression's TEMPLATE normal form (`${environment.apiUrl.replace('/api',
//! '')}/admin`), held apart from the literal bindings: only
//! [`fold_interpolations`] (and [`ConstTable::pinned_keys_in`], which walks
//! the same fold) reads it, never `get` / `resolve_expr*` / `resolve_identity`
//! / `candidates` / `entries`, so no consumer that mints identity from a value
//! (a queue topic) ever sees a `${…}`. The grammar is the TS parser's
//! `config_expr_text` over a text line: a dotted identifier path not rooted
//! at `this` / `super` and reading no env owner, a string literal, a
//! backtick template whose substitutions are such paths, `+` of those, and
//! `<path>.replace('<a>', '<b>')` with two string literals. Anything else (a
//! call, `this.x`, `process.env.X || ..`, a ternary) binds nothing.
//!
//! No panics: every slice goes through `get()` or lands on an ASCII byte.

use std::collections::{BTreeMap, BTreeSet};

/// Longest literal kept. Anything longer is a payload, not a name or a URL.
pub const MAX_VALUE_LEN: usize = 512;

/// Longest binding name (one dotted segment) accepted.
pub const MAX_NAME_LEN: usize = 128;

/// What [`fold_interpolations`] writes for a `${…}` span it could not resolve.
/// It still contains `${`, so the HTTP path normaliser keeps folding it to `{}`.
pub const UNRESOLVED_SPAN: &str = "${…}";

/// Bracket depth past which the object-literal walk gives up on a file.
const MAX_OBJECT_DEPTH: usize = 32;

/// DF.20: how deep [`fold_interpolations`] follows a config-expression
/// binding whose value holds `${…}` spans of its own (the call's template is
/// depth 0). Deeper, or back to a key already being folded, the span stays
/// unresolved.
const MAX_FOLD_DEPTH: usize = 3;

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
    /// Keys bound by [`ConstTable::pin`] (an overlay constant), not by source.
    pinned: BTreeSet<String>,
    /// DF.20: config-expression bindings in their template normal form
    /// (`ADMIN_BASE` -> `${environment.apiUrl.replace('/api', '')}/admin`),
    /// first binding wins. Read by the fold only (module doc).
    exprs: BTreeMap<String, String>,
    /// DF.20: extra distinct template values seen for an `exprs` key.
    expr_alternatives: BTreeMap<String, Vec<String>>,
}

impl ConstTable {
    /// Every literal binding `source` declares, and (TS, DF.20) every one-line
    /// `const|let|var` of a config expression (module doc). Line-oriented;
    /// `lang` is the engine's language tag and only selects which
    /// declaration shapes apply.
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
            // DF.20: a TS template with substitutions is no literal; the
            // config-expression reader may still bind it.
            let literal = binding(line, body, lang).filter(|(_, v)| !(ts && v.contains("${")));
            if let Some((name, value)) = literal {
                t.insert(name, value);
            } else if ts && let Some((name, value)) = config_binding(body) {
                t.insert_expr(name, &value);
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
        for (k, v) in &other.exprs {
            self.bind_expr(k, v);
        }
        for (k, alts) in &other.expr_alternatives {
            for v in alts {
                self.bind_expr(k, v);
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

    /// LA.4 (A11.7): resolve a queue-topic expression written in ANOTHER file
    /// than its binding. Only a constant-shaped key (dotted, or SCREAMING_CASE)
    /// is trusted across files — a lower-case `topic` bound as some function's
    /// local elsewhere must never name a parameter's topic here. The exact key
    /// resolves strictly, as [`Self::resolve_expr_strict`]; failing that, a
    /// dotted key whose LAST segment is SCREAMING_CASE resolves that segment
    /// strictly (Java `Topics.ORDERS`, where the scan bound bare `ORDERS`
    /// inside `class Topics`). An ambiguous key never resolves, and never
    /// falls through to its last segment.
    pub fn resolve_identity(&self, expr: &str) -> Option<&str> {
        let key = normalise_expr(expr)?;
        let dotted = key.contains('.');
        if !dotted && !screaming(&key) {
            return None;
        }
        if self.alternatives.contains_key(&key) {
            return None;
        }
        if let Some(v) = self.get(&key) {
            return Some(v);
        }
        let last = key
            .rsplit('.')
            .next()
            .filter(|seg| dotted && screaming(seg))?;
        if self.alternatives.contains_key(last) {
            return None;
        }
        self.get(last)
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

    /// CG.4a: every binding as `(key, value)`: each key's first value in key
    /// order, then every alternative value under its key, keys in order and
    /// alternatives in the order they were seen. Deterministic (BTreeMap), so
    /// a consumer that collects a set from it (the endpoint fold's configured
    /// sites) needs no sort of its own. A pinned key yields its one value.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        let first = self.by_key.iter().map(|(k, v)| (k.as_str(), v.as_str()));
        let rest = self
            .alternatives
            .iter()
            .flat_map(|(k, alts)| alts.iter().map(move |v| (k.as_str(), v.as_str())));
        first.chain(rest)
    }

    /// Distinct bound keys: literal bindings, plus the DF.20
    /// config-expression keys no literal binds (the `[const] repo table: <n>
    /// bindings` count).
    pub fn len(&self) -> usize {
        self.by_key.len()
            + self
                .exprs
                .keys()
                .filter(|k| !self.by_key.contains_key(*k))
                .count()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty() && self.exprs.is_empty()
    }

    /// Files that contributed at least one binding.
    pub fn files(&self) -> usize {
        self.files
    }

    /// Distinct extra values seen for already-bound keys. A pinned key is
    /// never a conflict: the pin is its one value.
    pub fn conflicts(&self) -> usize {
        self.alternatives
            .iter()
            .chain(&self.expr_alternatives)
            .filter(|(k, _)| !self.pinned.contains(*k))
            .map(|(_, alts)| alts.len())
            .sum()
    }

    /// LF.2d: bind `key` to `value` as an overlay constant. The same gates as
    /// a scanned binding run first: a secret-shaped name (`is_secret_name`)
    /// or a value `accept_value` refuses (empty, oversized, multi-line,
    /// interpolated) is NOT bound and returns `false`, so an overlay constant
    /// never bypasses secret redaction. Otherwise the pin REPLACES the key's
    /// binding, clears its alternatives (so [`Self::resolve_expr_strict`] and
    /// [`Self::resolve_identity`] see an unambiguous key) and marks it pinned.
    ///
    /// Apply pins AFTER every source merge: a later [`Self::merge_from`] into
    /// a pinned key would add its differing value as an alternative again and
    /// make the key ambiguous for the strict lookups.
    pub fn pin(&mut self, key: &str, value: &str) -> bool {
        if is_secret_name(key) {
            return false;
        }
        let Some(v) = accept_value(value) else {
            return false;
        };
        self.by_key.insert(key.to_string(), v);
        self.alternatives.remove(key);
        self.exprs.remove(key);
        self.expr_alternatives.remove(key);
        self.pinned.insert(key.to_string());
        true
    }

    /// `key` was bound by [`Self::pin`]. The hook a consumer that mints
    /// identity from a value (LA.4's queue-topic fold) reads to mark what an
    /// overlay constant produced.
    pub fn is_pinned(&self, key: &str) -> bool {
        self.pinned.contains(key)
    }

    /// The pinned keys [`fold_interpolations`] reads to fold `template`, in
    /// the order the fold reads them, each once: the leading span's lenient
    /// lookup (full key, then its last segment, as [`Self::resolve_expr`]),
    /// every later constant-shaped span's exact key, and (DF.20) every key a
    /// config-expression binding the fold followed read in turn. A span that
    /// did not resolve contributes nothing. Empty when no pin is involved.
    pub fn pinned_keys_in(&self, template: &str) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        if self.pinned.is_empty() {
            return out;
        }
        let mut reads = Vec::new();
        self.fold_at(template, 0, &mut Vec::new(), &mut reads);
        for key in reads {
            if let Some(pinned) = self.pinned.get(key.as_str())
                && !out.contains(&pinned.as_str())
            {
                out.push(pinned.as_str());
            }
        }
        out
    }

    /// One fold of `raw` at `depth`: the text with every resolvable span
    /// substituted and every other one [`UNRESOLVED_SPAN`], whether any span
    /// resolved, and whether EVERY span did. `stack` holds the
    /// config-expression keys being folded (a cycle guard); `reads` gains the
    /// key of each span that resolved, in order.
    fn fold_at(
        &self,
        raw: &str,
        depth: usize,
        stack: &mut Vec<String>,
        reads: &mut Vec<String>,
    ) -> Folded {
        let mut out = String::with_capacity(raw.len());
        let mut rest = raw;
        let mut hit = false;
        let mut complete = true;
        while let Some(start) = rest.find("${") {
            let leading = out.is_empty() && start == 0;
            out.push_str(rest.get(..start).unwrap_or(""));
            let inner = rest.get(start + 2..).unwrap_or("");
            let Some(end) = closing_brace(inner) else {
                out.push_str(rest.get(start..).unwrap_or(""));
                complete = false;
                rest = "";
                break;
            };
            let expr = inner.get(..end).unwrap_or("");
            let mut span_reads = Vec::new();
            match self.span_value(expr, leading, depth, stack, &mut span_reads) {
                Some(v) => {
                    out.push_str(&v);
                    reads.append(&mut span_reads);
                    hit = true;
                }
                None => {
                    out.push_str(UNRESOLVED_SPAN);
                    complete = false;
                }
            }
            rest = inner.get(end + 1..).unwrap_or("");
        }
        out.push_str(rest);
        Folded {
            text: out,
            hit,
            complete,
        }
    }

    /// The value of one `${expr}` span: `<path>.replace('<a>', '<b>')` is
    /// `<path>`'s value with the first `<a>` replaced by `<b>` (JS
    /// `String.prototype.replace` with a string pattern); anything else is
    /// the span's own value ([`Self::span_key`]). A config-expression value
    /// is folded in turn and counts only when every span of it resolved.
    ///
    /// A plain span reads the key's first binding, as the lenient lookup
    /// always has. A `.replace` span applies the transform to every
    /// deployment value of the key (A11.4: first binding first, then its
    /// alternatives) and takes the first result that is still a URL base
    /// ([`url_base_ok`]): `'https://api.kinaswap.com/api'.replace('/api',
    /// '')` is `https:/.kinaswap.com/api` in JS (the first `/api` is the
    /// host's), which names no service, so the fold reads the next
    /// deployment's (`http://localhost:3701/api` -> `http://localhost:3701`).
    /// No deployment giving a URL base leaves the span unresolved.
    fn span_value(
        &self,
        expr: &str,
        leading: bool,
        depth: usize,
        stack: &mut Vec<String>,
        reads: &mut Vec<String>,
    ) -> Option<String> {
        let (path, replace) = match split_replace(expr) {
            Some((path, from, to)) => (path, Some((from, to))),
            None => (expr, None),
        };
        let key = self.span_key(path, leading)?;
        // A literal binding wins over a config expression of the same key.
        let template = !self.by_key.contains_key(&key);
        let (map, alternatives) = if template {
            (&self.exprs, &self.expr_alternatives)
        } else {
            (&self.by_key, &self.alternatives)
        };
        let values = map
            .get(&key)
            .into_iter()
            .chain(alternatives.get(&key).into_iter().flatten())
            .take(if replace.is_some() { usize::MAX } else { 1 });
        for value in values {
            let mut nested = Vec::new();
            let resolved = if template {
                if depth >= MAX_FOLD_DEPTH || stack.contains(&key) {
                    return None;
                }
                stack.push(key.clone());
                let folded = self.fold_at(value, depth + 1, stack, &mut nested);
                stack.pop();
                if !folded.complete {
                    continue;
                }
                folded.text
            } else {
                value.clone()
            };
            let resolved = match replace {
                Some((from, to)) => {
                    let replaced = resolved.replacen(from, to, 1);
                    if !url_base_ok(&replaced) {
                        continue;
                    }
                    replaced
                }
                None => resolved,
            };
            reads.push(key);
            reads.append(&mut nested);
            return Some(resolved);
        }
        None
    }

    /// The key a span's path resolves through. The leading span (a base URL)
    /// is lenient, as [`Self::resolve_expr`]: the full normalised key, else
    /// its last segment, a literal binding before a config expression at each
    /// step. A later span must be constant-shaped and resolves exactly, to a
    /// key holding ONE value across both kinds of binding.
    fn span_key(&self, path: &str, leading: bool) -> Option<String> {
        let key = normalise_expr(path)?;
        let bound = |k: &str| self.by_key.contains_key(k) || self.exprs.contains_key(k);
        if leading {
            if bound(&key) {
                return Some(key);
            }
            let last = key.rsplit('.').next()?;
            return bound(last).then(|| last.to_string());
        }
        if !constant_shaped(path) {
            return None;
        }
        let values = |map: &BTreeMap<String, String>, alts: &BTreeMap<String, Vec<String>>| {
            usize::from(map.contains_key(&key)) + alts.get(&key).map_or(0, Vec::len)
        };
        let n = values(&self.by_key, &self.alternatives) + values(&self.exprs, &self.expr_alternatives);
        (n == 1).then_some(key)
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
        bind_first(&mut self.by_key, &mut self.alternatives, key, value);
    }

    /// DF.20: validate, redact, then bind a config expression's normal form.
    fn insert_expr(&mut self, name: &str, value: &str) {
        if is_secret_name(name) {
            return;
        }
        if let Some(v) = accept_expr_value(value) {
            self.bind_expr(name, &v);
        }
    }

    fn bind_expr(&mut self, key: &str, value: &str) {
        bind_first(&mut self.exprs, &mut self.expr_alternatives, key, value);
    }
}

/// First binding wins; a later distinct value is kept, once, as an
/// alternative.
fn bind_first(
    map: &mut BTreeMap<String, String>,
    alternatives: &mut BTreeMap<String, Vec<String>>,
    key: &str,
    value: &str,
) {
    match map.get(key) {
        None => {
            map.insert(key.to_string(), value.to_string());
        }
        Some(first) if first == value => {}
        Some(_) => {
            let alts = alternatives.entry(key.to_string()).or_default();
            if !alts.iter().any(|a| a == value) {
                alts.push(value.to_string());
            }
        }
    }
}

/// What [`ConstTable::fold_at`] made of one template.
struct Folded {
    text: String,
    /// At least one span resolved.
    hit: bool,
    /// Every span resolved (and every `${` closed).
    complete: bool,
}

/// Substitute every `${expr}` in `raw` the table can resolve. `Some` only if at
/// least one span resolved; unresolved spans become [`UNRESOLVED_SPAN`].
///
/// The LEADING span is a base URL and resolves leniently (the same leading-slot
/// inference as the HTTP resolver's BaseFold tier). A later span is usually a
/// path parameter, so it resolves strictly and only when it is constant-shaped
/// (dotted or ALL_CAPS): `/users/${id}` must stay a parameter even when some
/// file happens to bind `id`.
///
/// DF.20: a span `<path>.replace('<a>', '<b>')` resolves `<path>` by the same
/// rule, then replaces the first `<a>` by `<b>`. A span bound to a config
/// expression (`ADMIN_BASE` -> `${environment.apiUrl.replace('/api',
/// '')}/admin`) is folded in turn, at most [`MAX_FOLD_DEPTH`] levels deep,
/// each value's own first span leading; it resolves only when every span of
/// it does, and a key already being folded (a cycle) never does.
pub fn fold_interpolations(raw: &str, table: &ConstTable) -> Option<String> {
    let folded = table.fold_at(raw, 0, &mut Vec::new(), &mut Vec::new());
    folded.hit.then_some(folded.text)
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

// ---------------------------------------------------------------------------
// DF.20 config expressions (text reader; the TS parser's `config_expr_text`
// reads the same grammar off the AST)
// ---------------------------------------------------------------------------

/// TS `[export] const|let|var NAME[: T] = <config expression>` on one line:
/// NAME and the expression's template normal form. A class field (`readonly
/// base = …`, no `const` / `let` / `var`) is not one: the TS parser reads
/// those per class.
fn config_binding(body: &str) -> Option<(&str, String)> {
    let (head, rhs) = split_assign(body)?;
    let name = decl_name(head, "typescript")?;
    let declares = before_annotation(head)
        .split_whitespace()
        .any(|t| matches!(t, "const" | "let" | "var"));
    if !declares {
        return None;
    }
    let (value, rest) = config_expr(rhs)?;
    terminal(rest).then_some((name, value))
}

/// `term ('+' term)*`, concatenated.
fn config_expr(s: &str) -> Option<(String, &str)> {
    let (mut out, mut rest) = config_term(s.trim_start())?;
    loop {
        let r = rest.trim_start();
        let Some(next) = r.strip_prefix('+') else {
            return Some((out, r));
        };
        if next.starts_with(['+', '=']) {
            return None;
        }
        let (term, after) = config_term(next.trim_start())?;
        out.push_str(&term);
        rest = after;
    }
}

/// `( expr )` (then optionally `.replace(..)` when the parenthesised
/// expression is one path), a string literal, a backtick template of path
/// spans, or a path with an optional `.replace('<a>', '<b>')`.
fn config_term(s: &str) -> Option<(String, &str)> {
    match *s.as_bytes().first()? {
        b'(' => {
            let (inner, rest) = config_expr(s.get(1..)?)?;
            let rest = rest.trim_start().strip_prefix(')')?;
            match replace_call(rest) {
                Some((from, to, after)) => Some((replace_span(sole_path(&inner)?, from, to), after)),
                None => Some((inner, rest)),
            }
        }
        b'\'' | b'"' => {
            let (body, rest) = quoted(s)?;
            literal_piece_ok(body).then(|| (body.to_string(), rest))
        }
        b'`' => config_template(s),
        _ => {
            let (path, rest) = config_path(s)?;
            match replace_call(rest) {
                Some((from, to, after)) => Some((replace_span(&path, from, to), after)),
                None => Some((format!("${{{path}}}"), rest)),
            }
        }
    }
}

/// A backtick template whose substitutions are each a path (or a path's
/// `.replace(..)`), as its normal form: fragments verbatim, each
/// substitution `${path}`.
fn config_template(s: &str) -> Option<(String, &str)> {
    let mut rest = s.strip_prefix('`')?;
    let mut out = String::new();
    loop {
        let tick = rest.find('`')?;
        match rest.find("${").filter(|&sp| sp < tick) {
            Some(sp) => {
                let fragment = rest.get(..sp)?;
                literal_piece_ok(fragment).then_some(())?;
                out.push_str(fragment);
                let inner = rest.get(sp + 2..)?;
                let end = inner.find('}')?;
                out.push_str(&path_span(inner.get(..end)?.trim())?);
                rest = inner.get(end + 1..)?;
            }
            None => {
                let fragment = rest.get(..tick)?;
                literal_piece_ok(fragment).then_some(())?;
                out.push_str(fragment);
                return Some((out, rest.get(tick + 1..)?));
            }
        }
    }
}

/// One template substitution that is exactly a path or a path's
/// `.replace(..)`, as its normal-form span.
fn path_span(expr: &str) -> Option<String> {
    let (path, rest) = config_path(expr)?;
    match replace_call(rest) {
        Some((from, to, after)) => after
            .trim()
            .is_empty()
            .then(|| replace_span(&path, from, to)),
        None => rest.trim().is_empty().then(|| format!("${{{path}}}")),
    }
}

/// A dotted identifier path (`environment.apiUrl`, `env?.api.base`) at the
/// start of `s`, joined by `.`, and what follows it. At least two segments,
/// not rooted at `this` / `super`, and no `env` / `environ` owner (the
/// table never reads the environment). Stops before a `.replace(`.
fn config_path(s: &str) -> Option<(String, &str)> {
    let mut segs: Vec<&str> = Vec::new();
    let mut rest = s;
    loop {
        let end = ident_end(rest);
        let seg = rest.get(..end)?;
        if !valid_name(seg) {
            return None;
        }
        segs.push(seg);
        rest = rest.get(end..)?;
        let Some(after) = rest.strip_prefix("?.").or_else(|| rest.strip_prefix('.')) else {
            break;
        };
        if replace_call(rest).is_some() {
            break;
        }
        rest = after;
    }
    let path = segs.join(".");
    let rooted_ok = segs.len() >= 2 && !matches!(segs[0], "this" | "super");
    (rooted_ok && normalise_expr(&path).as_deref() == Some(path.as_str())).then_some((path, rest))
}

/// `.replace('<a>', '<b>')` at the start of `s`: both arguments string
/// literals [`replace_arg_ok`] accepts, and what follows the `)`.
fn replace_call(s: &str) -> Option<(&str, &str, &str)> {
    let rest = s
        .strip_prefix(".replace")?
        .trim_start()
        .strip_prefix('(')?
        .trim_start();
    let (from, rest) = quoted(rest)?;
    let rest = rest.trim_start().strip_prefix(',')?.trim_start();
    let (to, rest) = quoted(rest)?;
    let rest = rest.trim_start().strip_prefix(')')?;
    (replace_arg_ok(from) && replace_arg_ok(to)).then_some((from, to, rest))
}

/// A `.replace` argument the normal form can carry in single quotes and the
/// fold can evaluate exactly: no quote, backslash, brace, `$` (a JS
/// replacement pattern) or line break.
fn replace_arg_ok(arg: &str) -> bool {
    !arg.contains(['\'', '"', '`', '\\', '{', '}', '$', '\n', '\r'])
}

/// Literal text inside a normal form: nothing that could open a span or
/// hide an escape.
fn literal_piece_ok(piece: &str) -> bool {
    !piece.contains(['$', '`', '\\', '\n', '\r'])
}

/// The normal-form span of `path.replace('from', 'to')`.
fn replace_span(path: &str, from: &str, to: &str) -> String {
    format!("${{{path}.replace('{from}', '{to}')}}")
}

/// The path of a normal form that is exactly one `${path}` span.
fn sole_path(normal: &str) -> Option<&str> {
    let path = normal.strip_prefix("${")?.strip_suffix('}')?;
    (!path.contains(['$', '{', '}', '('])).then_some(path)
}

/// DF.20: a `.replace` result that is still a URL base. A value opening
/// with a web scheme (`http:`, `https:`, `ws:`, `wss:`, any case) must go on
/// with `//` and a non-empty authority; anything else (a path, a bare
/// segment, empty) is a base as it stands.
fn url_base_ok(v: &str) -> bool {
    let Some(colon) = v.find(':') else {
        return true;
    };
    let scheme = v.get(..colon).unwrap_or("");
    if !["http", "https", "ws", "wss"]
        .iter()
        .any(|s| scheme.eq_ignore_ascii_case(s))
    {
        return true;
    }
    v.get(colon + 1..)
        .and_then(|rest| rest.strip_prefix("//"))
        .is_some_and(|authority| !authority.is_empty() && !authority.starts_with(['/', '?', '#']))
}

/// A fold span `<path>.replace('<a>', '<b>')`: the path and both arguments.
fn split_replace(expr: &str) -> Option<(&str, &str, &str)> {
    let expr = expr.trim();
    expr.match_indices(".replace").find_map(|(i, _)| {
        let (from, to, rest) = replace_call(expr.get(i..)?)?;
        rest.trim()
            .is_empty()
            .then_some((expr.get(..i)?.trim(), from, to))
    })
}

/// A config expression's normal form worth binding: non-empty, not
/// oversized, one line, userinfo masked as a literal's.
fn accept_expr_value(v: &str) -> Option<String> {
    if v.is_empty() || v.len() > MAX_VALUE_LEN || v.contains(['\n', '\r']) {
        return None;
    }
    Some(mask_userinfo(v).unwrap_or_else(|| v.to_string()))
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
        let kt = scan("val url = \"$host/api\"\n", "kotlin");
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

    #[test]
    fn resolve_identity_gates_shape_and_ambiguity() {
        let mut repo = ConstTable::default();
        repo.merge_from(&scan(
            "export const ORDERS_TOPIC = 'orders';\n\
             export const Topics = {\n  PAYMENTS: 'payments',\n};\n\
             function f() {\n  const topic = 'audit-log';\n}\n\
             export const DUP = 'a';\n",
            "typescript",
        ));
        repo.merge_from(&scan(
            "public final class Topics {\n    public static final String ORDERS = \"orders\";\n}\n",
            "java",
        ));
        repo.merge_from(&scan("export const DUP = 'b';\n", "typescript"));
        // SCREAMING_CASE and dotted keys resolve across files.
        assert_eq!(repo.resolve_identity("ORDERS_TOPIC"), Some("orders"));
        assert_eq!(repo.resolve_identity("Topics.PAYMENTS"), Some("payments"));
        assert_eq!(repo.resolve_identity("this.ORDERS_TOPIC"), Some("orders"));
        // A dotted key falls back to its SCREAMING_CASE last segment (Java).
        assert_eq!(repo.resolve_identity("Topics.ORDERS"), Some("orders"));
        assert_eq!(repo.resolve_identity("Topics::ORDERS"), Some("orders"));
        // A lower-case binding never crosses files, even though it is bound.
        assert_eq!(repo.get("topic"), Some("audit-log"));
        assert_eq!(repo.resolve_identity("topic"), None);
        assert_eq!(repo.resolve_identity("this.topic"), None);
        // A lower-case last segment is not a fallback.
        assert_eq!(repo.resolve_identity("cfg.topic"), None);
        // Ambiguity refuses, exactly and through the last-segment fallback.
        assert_eq!(repo.resolve_identity("DUP"), None);
        assert_eq!(repo.resolve_identity("Consts.DUP"), None);
        // Env reads and non-identifiers never resolve.
        assert_eq!(repo.resolve_identity("process.env.ORDERS_TOPIC"), None);
        assert_eq!(repo.resolve_identity("PREFIX + x"), None);
        assert_eq!(repo.resolve_identity("UNBOUND"), None);
        // The lenient resolver still takes the first binding: the reason the
        // identity path does not use it.
        assert_eq!(repo.resolve_expr("DUP"), Some("a"));
    }

    /// LF.2d: a pin replaces a first-wins binding and clears its
    /// alternatives, so the strict lookups resolve it again; conflicts no
    /// longer count it.
    #[test]
    fn pin_overrides_first_wins_and_clears_alternatives() {
        let mut repo = ConstTable::default();
        repo.merge_from(&scan("export const GATEWAY = '/a';
", "typescript"));
        repo.merge_from(&scan("export const GATEWAY = '/b';
", "typescript"));
        assert_eq!(repo.resolve_expr_strict("GATEWAY"), None, "ambiguous before the pin");
        assert_eq!(repo.conflicts(), 1);
        assert!(!repo.is_pinned("GATEWAY"));

        assert!(repo.pin("GATEWAY", "/orders-svc"));
        assert!(repo.is_pinned("GATEWAY"));
        assert_eq!(repo.get("GATEWAY"), Some("/orders-svc"));
        assert_eq!(repo.resolve_expr_strict("GATEWAY"), Some("/orders-svc"));
        assert_eq!(repo.resolve_identity("GATEWAY"), Some("/orders-svc"));
        assert_eq!(repo.candidates("GATEWAY"), vec!["/orders-svc"]);
        assert_eq!(repo.conflicts(), 0);
        assert_eq!(
            fold_interpolations("${GATEWAY}/users", &repo).as_deref(),
            Some("/orders-svc/users")
        );

        // A key the source never bound is added.
        assert!(repo.pin("api.base", "http://gw:8080"));
        assert_eq!(repo.resolve_expr("this.api.base"), Some("http://gw:8080"));
        assert_eq!(repo.len(), 2);
    }

    /// The scan's gates run on a pin too: a secret-shaped name or a value the
    /// scan would refuse is not bound, and the table is left unchanged.
    #[test]
    fn secret_named_pin_is_refused() {
        let mut repo = scan("export const API = '/v1';
", "typescript");
        assert!(!repo.pin("API_TOKEN", "x"));
        assert!(!repo.pin("API", ""));
        assert!(!repo.pin("API", "${OTHER}/x"));
        assert!(!repo.pin("API", "a\nb"));
        assert_eq!(repo.get("API_TOKEN"), None);
        assert_eq!(repo.get("API"), Some("/v1"));
        assert!(!repo.is_pinned("API_TOKEN") && !repo.is_pinned("API"));
        assert_eq!(repo.len(), 1);
        // Userinfo is masked exactly as a scanned value's.
        assert!(repo.pin("GW", "https://user:pw@gw:8080/api"));
        assert!(!repo.get("GW").is_some_and(|v| v.contains("pw")));
    }

    /// `pinned_keys_in` names the pinned keys the fold reads: the leading
    /// span leniently (its last segment too), later constant-shaped spans
    /// exactly, never a path parameter and never an unpinned key.
    #[test]
    fn pinned_keys_in_finds_the_folded_pins() {
        let mut repo = scan("export const VERSION = 'v2';
export const PAGE = 'p';
", "typescript");
        assert!(repo.pinned_keys_in("${GATEWAY}/users").is_empty(), "no pins at all");
        assert!(repo.pin("GATEWAY", "/orders-svc"));
        assert!(repo.pin("id", "7"));
        assert!(repo.pin("PAGE", "q"));
        assert_eq!(repo.pinned_keys_in("${GATEWAY}/users"), ["GATEWAY"]);
        assert_eq!(repo.pinned_keys_in("${this.cfg.GATEWAY}/users"), ["GATEWAY"], "last segment");
        assert_eq!(repo.pinned_keys_in("${GATEWAY}/${VERSION}/users"), ["GATEWAY"], "VERSION is source");
        assert_eq!(repo.pinned_keys_in("${GATEWAY}/${PAGE}/${GATEWAY}"), ["GATEWAY", "PAGE"]);
        assert!(repo.pinned_keys_in("/users/${id}").is_empty(), "a path parameter never folds");
        assert!(repo.pinned_keys_in("/x/${GATEWAY}").iter().eq(["GATEWAY"].iter()), "a later span, exact");
        assert!(repo.pinned_keys_in("${OTHER}/users").is_empty());
        assert!(repo.pinned_keys_in("/users").is_empty());
        assert!(repo.pinned_keys_in("${GATEWAY").is_empty(), "unclosed span");
    }

    /// DF.20: a one-line TS `const|let|var` of a config expression binds its
    /// template normal form, apart from the literal bindings: the literal
    /// lookups never see it. A class field, a call, `this.x`, an env read, a
    /// fallback, a ternary or a bare identifier binds nothing.
    #[test]
    fn const_config_expression_binds() {
        let t = scan(
            "import { environment } from '../environments/environment';\n\
             export const ADMIN_BASE = environment.apiUrl.replace('/api', '') + '/admin';\n\
             const NOTICES: string = `${environment.apiUrl}/notifications`;\n\
             let plainBase = environment.api.base;\n\
             var paren = (environment.apiUrl).replace(\"/api\", '/v2') + '/x'; // c\n\
             export const LIT = '/a' + '/b';\n\
             class S {\n  private readonly base = environment.apiUrl.replace('/api', '') + '/admin';\n}\n\
             export const CALL = environment.build('/x');\n\
             export const SELF = this.cfg.apiUrl + '/x';\n\
             export const ENV = process.env.API_URL || 'http://localhost';\n\
             export const ENV2 = process.env.API_URL + '/x';\n\
             export const TERN = environment.production ? '/p' : '/d';\n\
             export const BARE = base + '/x';\n\
             export const SPLICE = `${base}/x`;\n\
             export const DOLLAR = environment.apiUrl.replace('/api', '$&');\n\
             export const API_TOKEN_URL = environment.apiUrl + '/token';\n",
            "typescript",
        );
        let expr = |k: &str| t.exprs.get(k).map(String::as_str);
        assert_eq!(
            expr("ADMIN_BASE"),
            Some("${environment.apiUrl.replace('/api', '')}/admin")
        );
        assert_eq!(expr("NOTICES"), Some("${environment.apiUrl}/notifications"));
        assert_eq!(expr("plainBase"), Some("${environment.api.base}"));
        assert_eq!(expr("paren"), Some("${environment.apiUrl.replace('/api', '/v2')}/x"));
        assert_eq!(expr("LIT"), Some("/a/b"));
        for k in [
            "base", "CALL", "SELF", "ENV", "ENV2", "TERN", "BARE", "SPLICE", "DOLLAR",
            "API_TOKEN_URL",
        ] {
            assert_eq!(expr(k), None, "{k} is no config expression");
        }
        // Never a literal binding: no identity consumer reads a template.
        for k in ["ADMIN_BASE", "NOTICES", "LIT"] {
            assert_eq!(t.get(k), None, "{k}");
            assert_eq!(t.resolve_expr(k), None, "{k}");
            assert_eq!(t.resolve_identity(k), None, "{k}");
            assert!(t.candidates(k).is_empty(), "{k}");
        }
        assert_eq!(t.entries().count(), 0);
        assert_eq!(t.len(), 5, "the [const] line counts them");
        assert_eq!(t.files(), 1);
        // Merging is first-wins with alternatives, as for a literal.
        let mut repo = ConstTable::default();
        repo.merge_from(&t);
        repo.merge_from(&scan(
            "export const ADMIN_BASE = environment.adminUrl + '/admin';\n",
            "typescript",
        ));
        assert_eq!(
            repo.exprs.get("ADMIN_BASE").map(String::as_str),
            Some("${environment.apiUrl.replace('/api', '')}/admin")
        );
        assert_eq!(repo.conflicts(), 1);
        // A pin replaces a config expression like a literal.
        assert!(repo.pin("ADMIN_BASE", "/pinned"));
        assert_eq!(repo.conflicts(), 0);
        assert_eq!(
            fold_interpolations("${ADMIN_BASE}/users", &repo).as_deref(),
            Some("/pinned/users")
        );
    }

    /// DF.20: `<path>.replace('<a>', '<b>')` replaces the FIRST occurrence
    /// (JS `String.prototype.replace` with a string pattern), in a call's own
    /// template and through a config-expression binding.
    #[test]
    fn fold_replace_first_occurrence() {
        let t = scan(
            "export const environment = {\n  apiUrl: 'http://localhost:3701/api',\n  twice: '/api/x/api',\n};\n\
             export const ADMIN_BASE = environment.apiUrl.replace('/api', '') + '/admin';\n",
            "typescript",
        );
        assert_eq!(
            fold_interpolations("${environment.apiUrl.replace('/api', '')}/admin/stats", &t).as_deref(),
            Some("http://localhost:3701/admin/stats")
        );
        assert_eq!(
            fold_interpolations("${environment.twice.replace('/api', '')}", &t).as_deref(),
            Some("/x/api"),
            "first occurrence only"
        );
        assert_eq!(
            fold_interpolations("${ environment.apiUrl.replace( \"/api\" , '/v2' ) }/x", &t).as_deref(),
            Some("http://localhost:3701/v2/x")
        );
        assert_eq!(
            fold_interpolations("${ADMIN_BASE}/users", &t).as_deref(),
            Some("http://localhost:3701/admin/users")
        );
        // `this.` and the last-segment fallback apply to the replaced path.
        assert_eq!(
            fold_interpolations("${this.apiUrl.replace('/api', '')}/x", &t).as_deref(),
            Some("http://localhost:3701/x")
        );
        // A pattern that is absent leaves the value as it is.
        assert_eq!(
            fold_interpolations("${environment.apiUrl.replace('/nope', '')}/x", &t).as_deref(),
            Some("http://localhost:3701/api/x")
        );
        // Anything but two plain string literals is no `.replace` span.
        for raw in [
            "${environment.apiUrl.replace(/api/, '')}/x",
            "${environment.apiUrl.replace('/api', x)}/x",
            "${environment.apiUrl.replace('/api', '$&')}/x",
            "${environment.apiUrl.replace('/api')}/x",
            "${environment.apiUrl.replace('/api', '').trim()}/x",
        ] {
            assert_eq!(fold_interpolations(raw, &t), None, "{raw}");
        }
        // A later span folds a `.replace` only on a constant-shaped path.
        assert_eq!(
            fold_interpolations("/p/${environment.apiUrl.replace('http://', '')}", &t).as_deref(),
            Some("/p/localhost:3701/api")
        );
        assert_eq!(
            fold_interpolations("/p/${id.replace('a', 'b')}", &t),
            None,
            "a lower-case later span is a parameter"
        );
    }

    /// DF.20: a `.replace` span reads every deployment value of its key and
    /// takes the first whose transform is still a URL base. Kina's
    /// production `https://api.kinaswap.com/api` loses its host's `/api` to
    /// `.replace('/api', '')` (JS replaces the FIRST occurrence), so the
    /// development binding is the one that names a service.
    #[test]
    fn fold_replace_skips_a_deployment_it_breaks() {
        let mut repo = ConstTable::default();
        for src in [
            "export const environment = {\n  apiUrl: 'https://api.kinaswap.com/api',\n};\n",
            "export const environment = {\n  apiUrl: 'https://api.kinaswap.com/api',\n};\n",
            "export const environment = {\n  apiUrl: 'http://localhost:3701/api',\n};\n",
            "export const ADMIN_BASE = environment.apiUrl.replace('/api', '') + '/admin';\n",
        ] {
            repo.merge_from(&scan(src, "typescript"));
        }
        assert_eq!(
            fold_interpolations("${environment.apiUrl.replace('/api', '')}/admin/stats", &repo).as_deref(),
            Some("http://localhost:3701/admin/stats")
        );
        assert_eq!(
            fold_interpolations("${ADMIN_BASE}/users", &repo).as_deref(),
            Some("http://localhost:3701/admin/users")
        );
        // A plain span still reads the first binding.
        assert_eq!(
            fold_interpolations("${environment.apiUrl}/x", &repo).as_deref(),
            Some("https://api.kinaswap.com/api/x")
        );
        // No deployment survives: unresolved.
        let only = scan(
            "export const environment = {\n  apiUrl: 'https://api.kinaswap.com/api',\n};\n",
            "typescript",
        );
        assert_eq!(
            fold_interpolations("${environment.apiUrl.replace('/api', '')}/admin", &only),
            None
        );
        assert_eq!(
            fold_interpolations("${environment.apiUrl.replace('api.kinaswap.com', '')}/x", &only),
            None,
            "an emptied authority is no base"
        );
        for (v, ok) in [
            ("http://localhost:3701", true),
            ("HTTPS://h/x", true),
            ("/api", true),
            ("", true),
            ("api/v1", true),
            ("localhost:3701/api", true),
            ("https:/.kinaswap.com/api", false),
            ("https://", false),
            ("wss:///x", false),
        ] {
            assert_eq!(url_base_ok(v), ok, "{v}");
        }
    }

    /// DF.20: a binding whose value holds spans of its own is folded in turn,
    /// three levels deep at most, and resolves only when every span of it
    /// does.
    #[test]
    fn fold_nested_template() {
        let t = scan(
            "export const environment = {\n  apiUrl: 'http://localhost:3701/api',\n};\n\
             export const ROOT = environment.apiUrl.replace('/api', '');\n\
             export const ADMIN = Consts.ROOT + '/admin';\n\
             export const NOTICES = `${environment.apiUrl}/notices`;\n\
             export const HALF = `${environment.apiUrl}/x/${environment.missing}`;\n\
             export const L1 = Consts.L2 + '/1';\n\
             export const L2 = Consts.L3 + '/2';\n\
             export const L3 = environment.apiUrl + '/3';\n\
             export const M1 = Consts.M2 + '/1';\n\
             export const M2 = Consts.M3 + '/2';\n\
             export const M3 = Consts.M4 + '/3';\n\
             export const M4 = environment.apiUrl + '/4';\n",
            "typescript",
        );
        assert_eq!(
            fold_interpolations("${ADMIN}/users", &t).as_deref(),
            Some("http://localhost:3701/admin/users"),
            "ADMIN -> ROOT -> environment.apiUrl"
        );
        assert_eq!(
            fold_interpolations("${NOTICES}/unread", &t).as_deref(),
            Some("http://localhost:3701/api/notices/unread")
        );
        assert_eq!(
            fold_interpolations("${HALF}/y", &t),
            None,
            "a binding that folds only in part is unresolved"
        );
        assert_eq!(
            fold_interpolations("${L1}/a", &t).as_deref(),
            Some("http://localhost:3701/api/3/2/1/a"),
            "three levels fold"
        );
        assert_eq!(
            fold_interpolations("${M1}/a", &t),
            None,
            "a fourth level is past MAX_FOLD_DEPTH"
        );
        // A later constant-shaped span folds a binding too, by its exact key.
        assert_eq!(
            fold_interpolations("/v/${NOTICES}", &t).as_deref(),
            Some("/v/http://localhost:3701/api/notices")
        );
        assert_eq!(fold_interpolations("/v/${Consts.NOTICES}", &t), None);
    }

    /// DF.20: a binding that reaches itself never resolves, at any length of
    /// cycle, and the rest of the template still folds.
    #[test]
    fn fold_cycle_stays_unresolved() {
        let t = scan(
            "export const environment = {\n  apiUrl: 'http://localhost:3701/api',\n};\n\
             export const LOOP = Consts.LOOP + '/x';\n\
             export const PING = Consts.PONG + '/a';\n\
             export const PONG = Consts.PING + '/b';\n",
            "typescript",
        );
        assert_eq!(fold_interpolations("${LOOP}/y", &t), None);
        assert_eq!(fold_interpolations("${PING}/y", &t), None);
        assert_eq!(
            fold_interpolations("${environment.apiUrl}/${Consts.LOOP}", &t).as_deref(),
            Some("http://localhost:3701/api/${…}")
        );
    }

    /// DF.20: literal bindings and every literal lookup are unchanged by the
    /// config-expression reader, and `pinned_keys_in` follows the fold into a
    /// config expression's own spans.
    #[test]
    fn literal_bindings_unchanged() {
        let src = "export const TOPIC = 'orders';\nexport const environment = {\n  apiUrl: 'http://api',\n};\n\
                   export const ADMIN = environment.apiUrl + '/admin';\n";
        let t = scan(src, "typescript");
        assert_eq!(t.get("TOPIC"), Some("orders"));
        assert_eq!(t.get("environment.apiUrl"), Some("http://api"));
        assert_eq!(t.resolve_identity("TOPIC"), Some("orders"));
        let literal: Vec<(&str, &str)> = t.entries().collect();
        assert_eq!(
            literal,
            [
                ("TOPIC", "orders"),
                ("apiUrl", "http://api"),
                ("environment.apiUrl", "http://api")
            ]
        );
        assert_eq!(
            fold_interpolations("${environment.apiUrl}/users/${id}", &t).as_deref(),
            Some("http://api/users/${…}")
        );
        let mut pinned = t.clone();
        assert!(pinned.pin("environment.apiUrl", "http://gw"));
        assert_eq!(pinned.pinned_keys_in("${ADMIN}/users"), ["environment.apiUrl"]);
        assert_eq!(
            fold_interpolations("${ADMIN}/users", &pinned).as_deref(),
            Some("http://gw/admin/users")
        );
        assert!(pinned.pinned_keys_in("${MISSING}/users").is_empty());
    }

    /// CG.4a: every binding, first values in key order, then each key's
    /// alternatives in the order they were seen, so a consumer sees every
    /// deployment's value deterministically. A pin is its key's one value.
    #[test]
    fn entries_yields_first_values_then_alternatives() {
        let mut repo = scan("export const environment = { apiUrl: 'http://localhost:3701' };\n", "typescript");
        for src in [
            "export const environment = { apiUrl: 'https://api.kinaswap.com' };\n",
            "export const environment = { apiUrl: 'https://staging.kinaswap.com' };\n",
            "export const API_BASE = 'https://b.io';\nconst a = 'x';\n",
            "export const API_BASE = 'https://c.io';\n",
        ] {
            repo.merge_from(&scan(src, "typescript"));
        }
        let got: Vec<(&str, &str)> = repo.entries().collect();
        assert_eq!(
            got,
            [
                ("API_BASE", "https://b.io"),
                ("a", "x"),
                // The object walk binds the member's bare name too.
                ("apiUrl", "http://localhost:3701"),
                ("environment.apiUrl", "http://localhost:3701"),
                ("API_BASE", "https://c.io"),
                ("apiUrl", "https://api.kinaswap.com"),
                ("apiUrl", "https://staging.kinaswap.com"),
                ("environment.apiUrl", "https://api.kinaswap.com"),
                ("environment.apiUrl", "https://staging.kinaswap.com"),
            ]
        );
        assert!(repo.pin("environment.apiUrl", "https://gw.io"));
        let got: Vec<(&str, &str)> = repo.entries().collect();
        assert_eq!(
            got,
            [
                ("API_BASE", "https://b.io"),
                ("a", "x"),
                ("apiUrl", "http://localhost:3701"),
                ("environment.apiUrl", "https://gw.io"),
                ("API_BASE", "https://c.io"),
                ("apiUrl", "https://api.kinaswap.com"),
                ("apiUrl", "https://staging.kinaswap.com"),
            ]
        );
        assert_eq!(ConstTable::default().entries().count(), 0);
    }
}
