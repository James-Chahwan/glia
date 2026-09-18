//! The `.glia/overlay.toml` schema and its loader. (LF.2a)
//!
//! ONE checked-in file holds three kinds of content, each applied by its own
//! consumer packet. This module is data only: it parses, validates and keeps
//! source spans; it never touches a graph.
//!
//! | section | kind | consumer | `--no-overlay` |
//! |---|---|---|---|
//! | `[walk]`, `[[project]]`, `[entrypoints]` | user config | LF.3a / LF.3b | still applied |
//! | `[[constraint]]`, `[[decision]]`, `[[note]]` | declared knowledge | LF.4a | still applied |
//! | `[constants]`, `[[route_prefix]]`, `[[wrapper]]`, `[[edge]]` | overlay (inference) | LF.2d / LF.2e / LF.2b | skipped |
//!
//! Loading never panics and never bails a build (pipeline stages emit empty):
//! - file absent -> `None`;
//! - TOML syntax error, schema error (unknown key, wrong type, missing
//!   required field) or `version != 1` -> the WHOLE config is defaulted and one
//!   error is recorded;
//! - a validation error drops ONLY the offending stanza (or list entry) and
//!   records an error naming its line.
//!
//! Every struct is `deny_unknown_fields`: a typo fails loudly instead of being
//! silently ignored. Only the repo-root `.glia/overlay.toml` is read; a nested
//! `.glia` is never walked (LF.1d), so sub-projects are addressed from the root
//! file by `scope` / `path`. The file reference is `docs/overlay.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::Path;

use repo_graph_core::{Confidence, EdgeCategoryId};
use serde::Deserialize;
/// Re-exported so consumers can name the span wrapper without depending on `toml`.
pub use toml::Spanned;

use crate::edge_category;

/// Repo-relative path of the file. Only the repo root's copy is read.
pub const OVERLAY_FILE: &str = ".glia/overlay.toml";
/// The one schema version this glia reads.
pub const SUPPORTED_VERSION: u32 = 1;
/// Upper bound for every `*_arg` positional index.
pub const MAX_ARG_INDEX: usize = 8;
/// Stanza ids: 1..=128 chars, no control chars (the external_inputs id rule).
pub const MAX_ID_CHARS: usize = 128;
/// `[[note]] text`: 1..=4096 chars (the CONV entry rule).
pub const MAX_NOTE_CHARS: usize = 4096;
/// Section names in `section_counts` / marker order.
pub const SECTIONS: [&str; 10] = [
    "walk", "project", "entrypoints", "constants", "route_prefix", "wrapper", "edge", "constraint",
    "decision", "note",
];
/// Edge categories an `[[edge]]` may never declare: structural (DEFINES /
/// CONTAINS) or owned by the git-history snapshot (CO_CHANGES).
pub const REJECTED_EDGE_CATEGORIES: &[EdgeCategoryId] =
    &[edge_category::DEFINES, edge_category::CONTAINS, edge_category::CO_CHANGES];
/// `[[wrapper]] kind` values.
pub const WRAPPER_KINDS: &[&str] = &["http", "queue_producer", "queue_consumer"];
/// `[[constraint]] kind` values.
pub const CONSTRAINT_KINDS: &[&str] = &["forbid_edge", "no_cycle", "invariant"];
/// Accepted fixed `[[wrapper]] method` values (case-insensitive).
pub const HTTP_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
/// Generated `[[note]]` ids are `note#<n>`; a declared id may not use the prefix.
pub const NOTE_ID_PREFIX: &str = "note#";

/// The whole file. Each field names the packet that applies it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GliaConfig {
    /// Must be [`SUPPORTED_VERSION`]; anything else defaults the whole config.
    #[serde(default)]
    pub version: u32,
    /// Extra walk skips (LF.3a).
    #[serde(default)]
    pub walk: WalkConfig,
    /// Declared sub-project roots (LF.3a).
    #[serde(default)]
    pub project: Vec<Spanned<ProjectDecl>>,
    /// Declared entrypoints (LF.3b).
    #[serde(default)]
    pub entrypoints: EntrypointsConfig,
    /// `NAME = "literal"` constant aliases pinned into the constant table (LF.2d).
    #[serde(default)]
    pub constants: BTreeMap<String, Spanned<String>>,
    /// Per-scope route mounts (LF.2d).
    #[serde(default)]
    pub route_prefix: Vec<Spanned<RoutePrefixDecl>>,
    /// Wrapper-call / client-receiver maps (LF.2e).
    #[serde(default)]
    pub wrapper: Vec<Spanned<WrapperDecl>>,
    /// Explicit edges (LF.2b).
    #[serde(default)]
    pub edge: Vec<Spanned<EdgeDecl>>,
    /// Declared rules -> CONSTRAINT cells (LF.4a).
    #[serde(default)]
    pub constraint: Vec<Spanned<ConstraintDecl>>,
    /// Declared decisions -> DECISION cells (LF.4a).
    #[serde(default)]
    pub decision: Vec<Spanned<DecisionDecl>>,
    /// Notes -> CONV cells (LF.4a).
    #[serde(default)]
    pub note: Vec<Spanned<NoteDecl>>,
}

/// `[walk]`: gitignore-syntax patterns anchored at the repo root. They only
/// extend the built-in skips; nothing here can un-skip a hard skip.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalkConfig {
    #[serde(default)]
    pub skip: Vec<String>,
}

/// `[[project]]`: an extra sub-project root the manifest scan cannot see.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDecl {
    pub path: String,
    #[serde(default)]
    pub label: Option<String>,
}

impl ProjectDecl {
    /// `path` without a leading `./` or `/` and without a trailing `/`.
    pub fn rel_path(&self) -> &str {
        let p = self.path.trim();
        let p = p.strip_prefix("./").unwrap_or(p);
        p.trim_start_matches('/').trim_end_matches('/')
    }
}

/// `[entrypoints]`: exact qnames, or `<prefix>::*` for the prefix's descendants.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntrypointsConfig {
    #[serde(default)]
    pub qnames: Vec<Spanned<String>>,
}

/// `[[route_prefix]]`: every ROUTE under `scope` is also reachable at `prefix + path`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutePrefixDecl {
    /// Project label or repo-relative path; `.` or `""` = the whole repo.
    pub scope: String,
    pub prefix: String,
    #[serde(default)]
    pub origin: Origin,
}

/// `[[wrapper]]`: call sites of `call(...)` (or, with `receiver = true`, of
/// `call.<verb>(...)`) are HTTP / queue sinks with the declared argument layout.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WrapperDecl {
    pub call: String,
    #[serde(default)]
    pub receiver: bool,
    /// One of [`WRAPPER_KINDS`]; see [`WrapperDecl::wrapper_kind`].
    pub kind: String,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub method_arg: Option<usize>,
    #[serde(default)]
    pub path_arg: Option<usize>,
    #[serde(default)]
    pub topic_arg: Option<usize>,
    #[serde(default)]
    pub broker: Option<String>,
    /// Optional filter on the engine's language names (`typescript`, `python`,
    /// ...). Empty = every language.
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub origin: Origin,
}

/// Typed `[[wrapper]] kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapperKind {
    Http,
    QueueProducer,
    QueueConsumer,
}

impl WrapperDecl {
    /// `None` only for a stanza the loader already dropped.
    pub fn wrapper_kind(&self) -> Option<WrapperKind> {
        match self.kind.as_str() {
            "http" => Some(WrapperKind::Http),
            "queue_producer" => Some(WrapperKind::QueueProducer),
            "queue_consumer" => Some(WrapperKind::QueueConsumer),
            _ => None,
        }
    }

    /// The path argument index: a receiver form defaults it to 0.
    pub fn path_arg_index(&self) -> Option<usize> {
        if self.receiver { Some(self.path_arg.unwrap_or(0)) } else { self.path_arg }
    }
}

/// `[[edge]]`: an explicit edge between two exact qnames.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeDecl {
    pub from: String,
    pub to: String,
    /// An `edge_category::ALL` name, never one of [`REJECTED_EDGE_CATEGORIES`].
    pub category: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub origin: Origin,
}

impl EdgeDecl {
    /// The category id; `None` only for a stanza the loader already dropped.
    pub fn category_id(&self) -> Option<EdgeCategoryId> {
        category_id(&self.category)
    }
}

/// `[[constraint]]`: a declared rule (the CONSTRAINT entry shape of
/// `external_inputs::validate_entry`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConstraintDecl {
    pub id: String,
    /// One of [`CONSTRAINT_KINDS`].
    pub kind: String,
    /// forbid_edge: required scope strings.
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    /// Edge category NAMES the rule is restricted to (empty = all).
    #[serde(default)]
    pub categories: Vec<String>,
    /// no_cycle / invariant: optional scope.
    #[serde(default)]
    pub scope: Option<String>,
    /// Exact qname to hang the cell on (else the scope's PROJECT node).
    #[serde(default)]
    pub anchor: Option<String>,
    /// invariant: required statement; other kinds: optional rationale.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub origin: Origin,
}

/// `[[decision]]`: a recorded decision; needs `title` or `text`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionDecl {
    pub id: String,
    #[serde(default)]
    pub anchor: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
}

/// `[[note]]`: a note on one node; `id` defaults to `note#<n>` (1-based).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteDecl {
    #[serde(default)]
    pub id: Option<String>,
    pub anchor: String,
    pub text: String,
    #[serde(default)]
    pub by: Option<String>,
}

/// Who authored a stanza. Defaults to `llm`: an unmarked stanza is inference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    #[default]
    Llm,
    Human,
}

impl Origin {
    /// The ORIGIN cell provenance string.
    pub fn provenance(self) -> &'static str {
        match self {
            Origin::Llm => "overlay:llm",
            Origin::Human => "overlay:human",
        }
    }

    /// The confidence an applied stanza carries: never Strong.
    pub fn confidence(self) -> Confidence {
        match self {
            Origin::Llm => Confidence::Weak,
            Origin::Human => Confidence::Medium,
        }
    }
}

/// A parsed, validated config plus the loader's errors and the line table
/// consumers use to record `.glia/overlay.toml:<line>` evidence.
#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: GliaConfig,
    /// One line each, prefixed `.glia/overlay.toml[:<line>]: `.
    pub errors: Vec<String>,
    line_starts: Vec<usize>,
}

impl LoadedConfig {
    /// 1-based line of `span.start` (a `[[section]]` stanza's span starts at its header).
    pub fn line_of(&self, span: Range<usize>) -> u32 {
        let idx = match self.line_starts.binary_search(&span.start) {
            Ok(i) => i + 1,
            Err(i) => i,
        };
        u32::try_from(idx.max(1)).unwrap_or(u32::MAX)
    }

    /// `.glia/overlay.toml:<line>` for a span: the `decl` value cells record.
    pub fn decl_of(&self, span: Range<usize>) -> String {
        format!("{OVERLAY_FILE}:{}", self.line_of(span))
    }

    /// Per-section entry counts in [`SECTIONS`] order: `walk` counts skip
    /// patterns, `entrypoints` qname patterns, `constants` keys, every other
    /// section its stanzas. Feeds the `[overlay] loaded` marker.
    pub fn section_counts(&self) -> [(&'static str, usize); 10] {
        let c = &self.config;
        let n = [
            c.walk.skip.len(),
            c.project.len(),
            c.entrypoints.qnames.len(),
            c.constants.len(),
            c.route_prefix.len(),
            c.wrapper.len(),
            c.edge.len(),
            c.constraint.len(),
            c.decision.len(),
            c.note.len(),
        ];
        std::array::from_fn(|i| (SECTIONS[i], n[i]))
    }

    /// True when no overlay (inference) section has an entry, so `--no-overlay`
    /// has nothing to switch off.
    pub fn is_overlay_empty(&self) -> bool {
        let c = &self.config;
        c.constants.is_empty() && c.route_prefix.is_empty() && c.wrapper.is_empty() && c.edge.is_empty()
    }

    fn at(&self, line: Option<u32>, msg: &str) -> String {
        match line {
            Some(l) => format!("{OVERLAY_FILE}:{l}: {msg}"),
            None => format!("{OVERLAY_FILE}: {msg}"),
        }
    }

    /// Drop every stanza whose check fails, recording one error per drop.
    fn keep<T>(
        &self,
        section: &str,
        items: Vec<Spanned<T>>,
        errors: &mut Vec<String>,
        mut check: impl FnMut(&T) -> Result<(), String>,
    ) -> Vec<Spanned<T>> {
        let mut kept = Vec::with_capacity(items.len());
        for item in items {
            match check(item.get_ref()) {
                Ok(()) => kept.push(item),
                Err(e) => {
                    let line = self.line_of(item.span());
                    errors.push(self.at(Some(line), &format!("[[{section}]] {e} (stanza dropped)")));
                }
            }
        }
        kept
    }

    fn validate(&mut self, text: &str) {
        let mut cfg = std::mem::take(&mut self.config);
        let mut errors = Vec::new();

        if cfg.version != SUPPORTED_VERSION {
            let msg = if cfg.version == 0 {
                "`version = 1` is missing; the whole file is ignored".to_string()
            } else {
                format!("version = {} is not supported (this glia reads version 1); the whole file is ignored", cfg.version)
            };
            self.errors.push(self.at(version_line(text), &msg));
            return;
        }

        cfg.walk.skip.retain(|p| match check_skip_pattern(p) {
            Ok(()) => true,
            Err(e) => {
                errors.push(self.at(None, &format!("[walk] skip {p:?}: {e} (pattern dropped)")));
                false
            }
        });

        let mut paths = BTreeSet::new();
        cfg.project = self.keep("project", cfg.project, &mut errors, |p| {
            let rel = p.rel_path();
            if rel.is_empty() || rel == "." {
                return Err(format!("path {:?} names the repo root", p.path));
            }
            if rel.split('/').any(|c| c == "..") {
                return Err(format!("path {:?} leaves the repo", p.path));
            }
            if p.label.as_deref().is_some_and(|l| l.trim().is_empty()) {
                return Err("label is empty".into());
            }
            if !paths.insert(rel.to_string()) {
                return Err(format!("duplicate path {rel:?}"));
            }
            Ok(())
        });

        let mut qnames = Vec::with_capacity(cfg.entrypoints.qnames.len());
        for q in std::mem::take(&mut cfg.entrypoints.qnames) {
            match check_entrypoint(q.get_ref()) {
                Ok(()) => qnames.push(q),
                Err(e) => {
                    let line = self.line_of(q.span());
                    errors.push(self.at(Some(line), &format!("[entrypoints] {e} (pattern dropped)")));
                }
            }
        }
        cfg.entrypoints.qnames = qnames;

        cfg.constants.retain(|k, v| {
            if is_constant_key(k) {
                return true;
            }
            let line = self.line_of(v.span());
            errors.push(self.at(
                Some(line),
                &format!("[constants] key {k:?} must match [A-Za-z_][A-Za-z0-9_.]* (dropped)"),
            ));
            false
        });

        cfg.route_prefix = self.keep("route_prefix", cfg.route_prefix, &mut errors, |r| {
            if !r.prefix.starts_with('/') || r.prefix.chars().any(char::is_whitespace) {
                return Err(format!("prefix {:?} must start with '/' and contain no whitespace", r.prefix));
            }
            Ok(())
        });

        cfg.wrapper = self.keep("wrapper", cfg.wrapper, &mut errors, check_wrapper);

        cfg.edge = self.keep("edge", cfg.edge, &mut errors, |e| {
            if e.from.trim().is_empty() || e.to.trim().is_empty() {
                return Err("from and to must be non-empty qnames".into());
            }
            match e.category_id() {
                None => Err(format!("category {:?} is not an edge category name", e.category)),
                Some(id) if REJECTED_EDGE_CATEGORIES.contains(&id) => Err(format!(
                    "category {:?} is not declarable (DEFINES / CONTAINS are structural, CO_CHANGES is history-owned)",
                    e.category
                )),
                Some(_) => Ok(()),
            }
        });

        let mut ids = BTreeSet::new();
        cfg.constraint = self.keep("constraint", cfg.constraint, &mut errors, |c| {
            check_id(&c.id)?;
            let present = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
            match c.kind.as_str() {
                "forbid_edge" if !(present(&c.from) && present(&c.to)) => {
                    return Err("kind forbid_edge needs `from` and `to`".into());
                }
                "invariant" if !present(&c.text) => return Err("kind invariant needs `text`".into()),
                k if !CONSTRAINT_KINDS.contains(&k) => {
                    return Err(format!("kind {k:?} is not one of {}", CONSTRAINT_KINDS.join(" | ")));
                }
                _ => {}
            }
            if let Some(bad) = c.categories.iter().find(|n| category_id(n).is_none()) {
                return Err(format!("category {bad:?} is not an edge category name"));
            }
            unique(&mut ids, &c.id)
        });

        let mut ids = BTreeSet::new();
        cfg.decision = self.keep("decision", cfg.decision, &mut errors, |d| {
            check_id(&d.id)?;
            let present = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
            if !present(&d.title) && !present(&d.text) {
                return Err("needs `title` or `text`".into());
            }
            unique(&mut ids, &d.id)
        });

        let mut ids = BTreeSet::new();
        cfg.note = self.keep("note", cfg.note, &mut errors, |n| {
            if n.anchor.trim().is_empty() {
                return Err("anchor must be a non-empty qname".into());
            }
            let chars = n.text.chars().count();
            if n.text.trim().is_empty() || chars > MAX_NOTE_CHARS {
                return Err(format!("text must be 1..={MAX_NOTE_CHARS} chars (got {chars})"));
            }
            match &n.id {
                Some(id) if id.starts_with(NOTE_ID_PREFIX) => {
                    Err(format!("id {id:?} uses the reserved {NOTE_ID_PREFIX:?} prefix"))
                }
                Some(id) => {
                    check_id(id)?;
                    unique(&mut ids, id)
                }
                None => Ok(()),
            }
        });

        self.config = cfg;
        self.errors.extend(errors);
    }
}

/// Read `<repo_root>/.glia/overlay.toml`. `None` when the file is absent; any
/// read, TOML or schema failure is `Some` with a defaulted config and one error.
pub fn load(repo_root: &Path) -> Option<LoadedConfig> {
    let path = repo_root.join(OVERLAY_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => return Some(failed(format!("{OVERLAY_FILE}: cannot read: {e}"))),
    };
    match String::from_utf8(bytes) {
        Ok(text) => Some(parse_str(&text)),
        Err(_) => Some(failed(format!("{OVERLAY_FILE}: not valid UTF-8; the whole file is ignored"))),
    }
}

/// Parse and validate file contents (what [`load`] does after reading).
pub fn parse_str(text: &str) -> LoadedConfig {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut line_starts = vec![0];
    line_starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
    let mut loaded = LoadedConfig { config: GliaConfig::default(), errors: Vec::new(), line_starts };
    match toml::from_str::<GliaConfig>(text) {
        Ok(cfg) => {
            loaded.config = cfg;
            loaded.validate(text);
        }
        Err(e) => {
            let line = e.span().map(|s| loaded.line_of(s));
            let msg = e.message().replace('\n', "; ");
            loaded.errors.push(loaded.at(line, &format!("{msg}; the whole file is ignored")));
        }
    }
    loaded
}

fn failed(msg: String) -> LoadedConfig {
    LoadedConfig { config: GliaConfig::default(), errors: vec![msg], line_starts: vec![0] }
}

fn category_id(name: &str) -> Option<EdgeCategoryId> {
    edge_category::ALL.iter().find(|(_, n)| *n == name).map(|(id, _)| *id)
}

/// Line of a top-level `version = ...` key (before the first table header).
fn version_line(text: &str) -> Option<u32> {
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with('[') {
            return None;
        }
        if t.strip_prefix("version").is_some_and(|r| r.trim_start().starts_with('=')) {
            return u32::try_from(i + 1).ok();
        }
    }
    None
}

fn check_skip_pattern(p: &str) -> Result<(), String> {
    if p.trim().is_empty() {
        return Err("empty pattern".into());
    }
    ignore::gitignore::GitignoreBuilder::new("/")
        .add_line(None, p)
        .map(|_| ())
        .map_err(|e| format!("not a valid gitignore pattern: {e}"))
}

fn check_entrypoint(q: &str) -> Result<(), String> {
    if q.is_empty() || q.chars().any(char::is_whitespace) {
        return Err(format!("qname {q:?} must be non-empty with no whitespace"));
    }
    let head = q.strip_suffix("::*").unwrap_or(q);
    if head.is_empty() || head.contains('*') {
        return Err(format!("qname {q:?} must be exact or end in `<prefix>::*`"));
    }
    Ok(())
}

fn is_constant_key(k: &str) -> bool {
    let mut chars = k.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

fn check_id(id: &str) -> Result<(), String> {
    let n = id.chars().count();
    if n == 0 || n > MAX_ID_CHARS || id.chars().any(char::is_control) {
        return Err(format!("id {id:?} must be 1..={MAX_ID_CHARS} chars with no control chars"));
    }
    Ok(())
}

fn unique(seen: &mut BTreeSet<String>, id: &str) -> Result<(), String> {
    if seen.insert(id.to_string()) { Ok(()) } else { Err(format!("duplicate id {id:?}")) }
}

fn check_wrapper(w: &WrapperDecl) -> Result<(), String> {
    if w.call.is_empty() || w.call.chars().any(|c| c.is_whitespace() || c == '(' || c == ')') {
        return Err(format!("call {:?} must be a bare callee name (no whitespace or parens)", w.call));
    }
    let Some(kind) = w.wrapper_kind() else {
        return Err(format!("kind {:?} is not one of {}", w.kind, WRAPPER_KINDS.join(" | ")));
    };
    for (name, arg) in [("method_arg", w.method_arg), ("path_arg", w.path_arg), ("topic_arg", w.topic_arg)] {
        if arg.is_some_and(|a| a > MAX_ARG_INDEX) {
            return Err(format!("{name} must be <= {MAX_ARG_INDEX}"));
        }
    }
    if let Some(l) = w.languages.iter().find(|l| {
        let mut c = l.chars();
        !(c.next().is_some_and(|c| c.is_ascii_lowercase())
            && c.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
    }) {
        return Err(format!("language {l:?} is not an engine language name"));
    }
    if let Some(m) = &w.method
        && !HTTP_METHODS.contains(&m.to_ascii_uppercase().as_str())
    {
        return Err(format!("method {m:?} is not one of {}", HTTP_METHODS.join(" | ")));
    }
    match kind {
        WrapperKind::Http => {
            if w.topic_arg.is_some() || w.broker.is_some() {
                return Err("kind http takes no topic_arg / broker".into());
            }
            if w.receiver {
                if w.method.is_some() || w.method_arg.is_some() {
                    return Err("receiver = true takes no method / method_arg (the member verb is the method)".into());
                }
            } else {
                if w.path_arg.is_none() {
                    return Err("kind http needs path_arg (or receiver = true)".into());
                }
                if w.method.is_some() == w.method_arg.is_some() {
                    return Err("kind http needs exactly one of method / method_arg".into());
                }
            }
        }
        WrapperKind::QueueProducer | WrapperKind::QueueConsumer => {
            if w.receiver {
                return Err("receiver = true requires kind http".into());
            }
            if w.method.is_some() || w.method_arg.is_some() || w.path_arg.is_some() {
                return Err(format!("kind {} takes no method / method_arg / path_arg", w.kind));
            }
            if w.topic_arg.is_none() {
                return Err(format!("kind {} needs topic_arg", w.kind));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"version = 1

[walk]
skip = ["legacy"]

[[project]]
path = "tools/migrator"
label = "migrator"

[entrypoints]
qnames = ["app::jobs::*"]

[constants]
GATEWAY = "/orders-svc"

[[route_prefix]]
scope = "orders"
prefix = "/orders-svc"

[[wrapper]]
call = "request"
kind = "http"
method_arg = 0
path_arg = 1

[[edge]]
from = "web::src::report::loadReport"
to = "api::report::build_report"
category = "CALLS"
note = "fetch through a dynamic URL"
origin = "human"

[[constraint]]
id = "web-no-db"
kind = "forbid_edge"
from = "web"
to = "services/api"
categories = ["CALLS"]

[[decision]]
id = "adr-7"
scope = "services/api"
title = "Charges are idempotent"
status = "accepted"

[[note]]
anchor = "services::api::app::charge"
text = "retries are safe"
by = "james"
"#;

    fn edge_at_line(line: usize, category: &str) -> String {
        let mut s = String::from("version = 1\n");
        for _ in 2..line {
            s.push_str("# pad\n");
        }
        s.push_str(&format!("[[edge]]\nfrom = \"a::f\"\nto = \"b::g\"\ncategory = \"{category}\"\n"));
        s
    }

    #[test]
    fn parses_every_section() {
        let l = parse_str(FULL);
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        for (name, n) in l.section_counts() {
            assert_eq!(n, 1, "section {name}");
        }
        assert_eq!(l.config.edge[0].get_ref().origin, Origin::Human);
        assert_eq!(l.config.edge[0].get_ref().category_id(), Some(edge_category::CALLS));
        assert_eq!(l.config.wrapper[0].get_ref().origin, Origin::Llm);
        assert!(!l.is_overlay_empty());
    }

    /// docs/overlay.md's example is the schema repo-graph targets: it must load clean.
    #[test]
    fn docs_example_parses_cleanly() {
        let doc = include_str!("../../docs/overlay.md");
        let body = doc.split("```toml\n").nth(1).and_then(|s| s.split("```").next()).expect("toml block");
        let l = parse_str(body);
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        let counts: Vec<usize> = l.section_counts().iter().map(|(_, n)| *n).collect();
        assert_eq!(counts, [2, 1, 2, 2, 1, 3, 1, 1, 1, 1]);
    }

    #[test]
    fn typo_in_stanza_is_an_error_not_silence() {
        let l = parse_str("version = 1\n[[edge]]\nform = \"a::f\"\nto = \"b::g\"\ncategory = \"CALLS\"\n");
        assert_eq!(l.errors.len(), 1, "{:?}", l.errors);
        assert!(l.errors[0].contains(":3:") && l.errors[0].contains("form"), "{}", l.errors[0]);
        assert!(l.config.edge.is_empty());
    }

    #[test]
    fn structural_category_is_rejected() {
        for cat in ["DEFINES", "CONTAINS", "CO_CHANGES", "NOPE"] {
            let mut text = edge_at_line(4, cat);
            text.push_str("[[edge]]\nfrom = \"a::f\"\nto = \"b::g\"\ncategory = \"HTTP_CALLS\"\n");
            let l = parse_str(&text);
            assert_eq!(l.errors.len(), 1, "{cat}: {:?}", l.errors);
            assert!(l.errors[0].starts_with(".glia/overlay.toml:4: ") && l.errors[0].contains(cat), "{}", l.errors[0]);
            assert_eq!(l.config.edge.len(), 1, "{cat}: only the valid stanza survives");
            assert_eq!(l.config.edge[0].get_ref().category, "HTTP_CALLS");
        }
    }

    #[test]
    fn wrong_version_defaults_everything() {
        for (text, line) in [
            (FULL.replacen("version = 1", "version = 2", 1), ":1: "),
            (FULL.replacen("version = 1", "", 1), "toml: "),
        ] {
            let l = parse_str(&text);
            assert_eq!(l.errors.len(), 1, "{:?}", l.errors);
            assert!(l.errors[0].contains(line) && l.errors[0].contains("version"), "{}", l.errors[0]);
            assert!(l.section_counts().iter().all(|(_, n)| *n == 0));
        }
    }

    #[test]
    fn malformed_toml_never_panics() {
        for text in ["version = 1\n[[edge]\n", "[[edge]]\nfrom = 3\n", "\u{feff}version = \"1\"", "", "\n\n"] {
            let l = parse_str(text);
            assert_eq!(l.errors.len(), 1, "{text:?}: {:?}", l.errors);
            assert!(l.section_counts().iter().all(|(_, n)| *n == 0));
        }
    }

    #[test]
    fn line_of_reports_the_stanza_line() {
        let l = parse_str(&edge_at_line(7, "CALLS"));
        assert!(l.errors.is_empty(), "{:?}", l.errors);
        let span = l.config.edge[0].span();
        assert_eq!(l.line_of(span.clone()), 7);
        assert_eq!(l.decl_of(span), ".glia/overlay.toml:7");
        let q = parse_str("version = 1\n[entrypoints]\nqnames = [\n  \"a::b\",\n  \"c::*\",\n]\n");
        let lines: Vec<u32> = q.config.entrypoints.qnames.iter().map(|s| q.line_of(s.span())).collect();
        assert_eq!(lines, [4, 5]);
    }

    fn scratch_repo(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("glia_overlay_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".glia")).expect("mkdir");
        d
    }

    #[test]
    fn absent_file_is_none() {
        let d = scratch_repo("absent");
        assert!(load(&d).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn present_file_loads_and_bad_bytes_are_an_error() {
        let d = scratch_repo("present");
        std::fs::write(d.join(OVERLAY_FILE), FULL).expect("write");
        let l = load(&d).expect("present");
        assert!(l.errors.is_empty() && l.config.edge.len() == 1, "{:?}", l.errors);
        std::fs::write(d.join(OVERLAY_FILE), [0xff_u8, 0xfe, 0x00]).expect("write");
        assert_eq!(load(&d).expect("present").errors.len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn wrapper_stanzas_are_validated() {
        let ok = [
            "call = \"api\"\nkind = \"http\"\nreceiver = true",
            "call = \"api.request\"\nkind = \"http\"\nmethod = \"get\"\npath_arg = 0\nlanguages = [\"typescript\"]",
            "call = \"publish\"\nkind = \"queue_producer\"\ntopic_arg = 0\nbroker = \"nats\"",
        ];
        let bad = [
            ("call = \"api\"\nkind = \"http\"\nreceiver = true\nmethod = \"GET\"", "receiver"),
            ("call = \"request\"\nkind = \"http\"\nmethod_arg = 0", "path_arg"),
            ("call = \"request\"\nkind = \"http\"\nmethod = \"GET\"\nmethod_arg = 0\npath_arg = 1", "exactly one"),
            ("call = \"request\"\nkind = \"http\"\nmethod = \"FETCH\"\npath_arg = 1", "FETCH"),
            ("call = \"publish\"\nkind = \"queue_consumer\"", "topic_arg"),
            ("call = \"publish\"\nkind = \"queue_producer\"\ntopic_arg = 9", "<= 8"),
            ("call = \"publish\"\nkind = \"grpc\"\ntopic_arg = 0", "grpc"),
            ("call = \"publish\"\nkind = \"queue_producer\"\nreceiver = true\ntopic_arg = 0", "receiver"),
            ("call = \"req uest\"\nkind = \"http\"\nreceiver = true", "bare callee"),
            ("call = \"api\"\nkind = \"http\"\nreceiver = true\nlanguages = [\"TypeScript\"]", "language"),
        ];
        for body in ok {
            let l = parse_str(&format!("version = 1\n[[wrapper]]\n{body}\n"));
            assert!(l.errors.is_empty(), "{body}: {:?}", l.errors);
        }
        for (body, needle) in bad {
            let l = parse_str(&format!("version = 1\n[[wrapper]]\n{body}\n"));
            assert!(l.config.wrapper.is_empty(), "{body}");
            assert_eq!(l.errors.len(), 1, "{body}: {:?}", l.errors);
            assert!(l.errors[0].contains(":2: [[wrapper]]") && l.errors[0].contains(needle), "{}", l.errors[0]);
        }
        let r = parse_str(&format!("version = 1\n[[wrapper]]\n{}\n", ok[0]));
        let w = r.config.wrapper[0].get_ref();
        assert_eq!((w.wrapper_kind(), w.path_arg_index()), (Some(WrapperKind::Http), Some(0)));
    }

    #[test]
    fn declared_knowledge_is_validated() {
        let text = r#"version = 1
[[constraint]]
id = "a"
kind = "forbid_edge"
from = "web"
[[constraint]]
id = "b"
kind = "invariant"
[[constraint]]
id = "c"
kind = "no_cycle"
categories = ["calls"]
[[constraint]]
id = "d"
kind = "no_cycle"
[[constraint]]
id = "d"
kind = "invariant"
text = "x"
[[decision]]
id = "adr-1"
status = "accepted"
[[note]]
id = "note#1"
anchor = "a::b"
text = "x"
[[note]]
anchor = "a::b"
text = ""
[[note]]
anchor = "a::b"
text = "kept"
"#;
        let l = parse_str(text);
        let lines: Vec<&str> = l.errors.iter().map(|e| e.split(": ").next().unwrap_or("")).collect();
        assert_eq!(
            lines,
            [
                ".glia/overlay.toml:2",
                ".glia/overlay.toml:6",
                ".glia/overlay.toml:9",
                ".glia/overlay.toml:16",
                ".glia/overlay.toml:20",
                ".glia/overlay.toml:23",
                ".glia/overlay.toml:27",
            ],
            "{:#?}",
            l.errors
        );
        assert_eq!(l.config.constraint.len(), 1);
        assert_eq!(l.config.constraint[0].get_ref().id, "d");
        assert!(l.config.decision.is_empty());
        assert_eq!(l.config.note.len(), 1);
    }

    #[test]
    fn user_config_and_constants_are_validated() {
        let text = r#"version = 1
[walk]
skip = ["legacy", "{a,b", ""]
[[project]]
path = "./tools/migrator/"
[[project]]
path = "tools/migrator"
[[project]]
path = "../outside"
[entrypoints]
qnames = ["app::jobs::*", "app::*::x", "*"]
[constants]
"api.base_URL" = "/api"
1BAD = "/x"
[[route_prefix]]
scope = "."
prefix = "orders"
"#;
        let l = parse_str(text);
        assert_eq!(l.errors.len(), 8, "{:#?}", l.errors);
        assert_eq!(l.config.walk.skip, ["legacy"]);
        assert_eq!(l.config.project.len(), 1);
        assert_eq!(l.config.project[0].get_ref().rel_path(), "tools/migrator");
        assert_eq!(l.config.entrypoints.qnames.len(), 1);
        assert_eq!(l.config.constants.keys().collect::<Vec<_>>(), ["api.base_URL"]);
        assert!(l.config.route_prefix.is_empty());
        assert!(!l.is_overlay_empty(), "the surviving constant is overlay content");
    }

    #[test]
    fn overlay_emptiness_and_origin() {
        let l = parse_str("version = 1\n[walk]\nskip = [\"x\"]\n[[decision]]\nid = \"d\"\ntext = \"t\"\n");
        assert!(l.errors.is_empty() && l.is_overlay_empty(), "{:?}", l.errors);
        assert_eq!((Origin::Llm.provenance(), Origin::Llm.confidence()), ("overlay:llm", Confidence::Weak));
        assert_eq!((Origin::Human.provenance(), Origin::Human.confidence()), ("overlay:human", Confidence::Medium));
        let bad = parse_str("version = 1\n[[edge]]\nfrom = \"a\"\nto = \"b\"\ncategory = \"CALLS\"\norigin = \"robot\"\n");
        assert_eq!(bad.errors.len(), 1);
        assert!(bad.config.edge.is_empty());
    }
}
