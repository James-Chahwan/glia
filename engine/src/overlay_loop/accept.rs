//! The only writer of `.glia/overlay.toml` (CE.3d): chosen candidate
//! stanzas in, orphaned / redundant rules out by gap id, validated by the
//! loader, written atomically, shown as a diff.
//!
//! [`accept`] runs, in order:
//! 1. BASE: the file's text, or none (a new file starts from `version = 1`).
//! 2. ADD: with a candidate, [`parse_candidate`] + [`merge`] (CE.3b) of the
//!    stanzas `only` names (their [`StanzaRef`] displays: `wrapper#1`,
//!    `constants.GATEWAY`; empty = every stanza). An unknown ref is an `Err`
//!    listing the candidate's refs. A constant or pattern the base already
//!    holds is a duplicate: nothing written for it.
//! 3. REMOVE: with gap ids, the repo is built with the file as it is (nothing
//!    persisted, no parse cache written) and [`gaps_report`] run with its
//!    root. Each id must be an `orphaned_rule` or `redundant_rule` row (an id
//!    no row has, or a row of another category, is an `Err` naming it). The
//!    row's stanza is found in BASE by the row's section and line (the text
//!    the report read) only to learn its IDENTITY - an `[[edge]]`'s from / to
//!    / category, a `[[constraint]]` / `[[decision]]` / `[[note]]` id (an
//!    id-less note: its anchor and text), an `[entrypoints]` pattern - and
//!    every entry with that identity is deleted from the document, never a
//!    line range. Two identical stanzas (twin rows, one gap) both go, both
//!    counted. Rule rows name only those sections: a `[[wrapper]]`,
//!    `[[route_prefix]]` or `[constants]` entry is never an orphaned or
//!    redundant rule, so it has no removal identity.
//! 4. VALIDATE: the new text must load (`glia_config::parse_str`) with no
//!    error BASE did not have, and with BASE's section counts plus the
//!    stanzas added minus the loader-kept stanzas removed; else `Err` and
//!    nothing is written.
//! 5. WRITE: unless `dry_run`, or the text is unchanged,
//!    `<repo>/.glia/overlay.toml.<pid>.tmp` then a rename over the file
//!    (`.glia/` created when absent; a `*.tmp` under `.glia` is outside the
//!    input fingerprint). Two accepts at once race on the rename: the loser's
//!    write is lost, never torn.
//!
//! [`AcceptSummary::diff`] is a unified diff of BASE -> new (`--- /dev/null`
//! for a new file), from a line LCS written here, cut at 400 lines.
//!
//! Marker (the fired_on line), once per accepted call (a refusal prints none):
//! `[overlay] accept repo=<repo> added=<a> (route_prefix=<p> wrapper=<w> edge=<e> constants=<c> entrypoints=<n>) removed=<r> duplicates=<u> file=.glia/overlay.toml dry_run=<bool> surface=<cli|py|engine>`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::Serialize;
use toml_edit::{DocumentMut, Item, TableLike, Value};

use glia_code_domain::glia_config::{self, LoadedConfig, OVERLAY_FILE};

use super::propose::{build_quiet, repo_roots};
use super::writer::{Candidate, Location, StanzaRef, merge, parse_candidate};
use crate::gaps::{GapRow, GapsOptions, ORPHANED_RULE, REDUNDANT_RULE, gaps_report};

/// What a repo with no `.glia/overlay.toml` is measured against.
const EMPTY_OVERLAY: &str = "version = 1\n";
/// The candidate sections, in marker order.
const MARKER_SECTIONS: [&str; 5] = [
    "route_prefix",
    "wrapper",
    "edge",
    "constants",
    "entrypoints",
];
/// [`AcceptSummary::diff`] is cut at this many lines.
const MAX_DIFF_LINES: usize = 400;
/// Unchanged lines shown around a change.
const DIFF_CONTEXT: usize = 3;
/// A differing middle larger than this (old x new lines) is diffed as
/// all-removed then all-added instead of by LCS.
const MAX_LCS_CELLS: usize = 4_000_000;

/// How [`accept`] runs. Made with `Default` plus field assignment.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcceptOptions {
    /// The candidate stanzas to write, by [`StanzaRef`] display (`wrapper#1`,
    /// `constants.GATEWAY`); empty = every stanza.
    pub only: Vec<String>,
    /// Gap ids (`gap:<16 hex>`) of `orphaned_rule` / `redundant_rule` rows
    /// whose stanzas to remove.
    pub remove: Vec<String>,
    /// Validate and diff, write nothing.
    pub dry_run: bool,
    /// The `surface=` of the `[overlay] accept` and `[gaps]` markers (`cli`,
    /// `py`); empty = `engine`.
    pub surface: &'static str,
}

/// [`accept`]'s answer.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct AcceptSummary {
    /// Stanzas written, per candidate section (sections with none left out).
    pub added: BTreeMap<&'static str, usize>,
    /// Entries deleted from the file (twins counted each).
    pub removed: usize,
    /// Chosen stanzas the file already held (nothing written for them).
    pub duplicates: usize,
    /// The overlay file's path (`<repo>/.glia/overlay.toml`).
    pub file: String,
    pub dry_run: bool,
    /// The file was (re)written: false on a dry run or when nothing changed.
    pub written: bool,
    /// Unified diff of the file before -> after; empty when nothing changed.
    pub diff: String,
}

/// Accept into `repo_path`'s `.glia/overlay.toml`: `candidate_text`'s chosen
/// stanzas added and `opts.remove`'s rule rows removed, validated, written:
/// see the module doc. `Err` (nothing written) when there is nothing to
/// accept, the candidate is refused, a ref or gap id is unknown, the file
/// cannot be read, the new text would not load as validated, or the write
/// fails.
pub fn accept(
    repo_path: &str,
    candidate_text: Option<&str>,
    opts: &AcceptOptions,
) -> Result<AcceptSummary, String> {
    if candidate_text.is_none() && opts.remove.is_empty() {
        return Err(
            "overlay accept: nothing to accept (give a candidate, gap ids to remove, or both)"
                .to_string(),
        );
    }
    if candidate_text.is_none() && !opts.only.is_empty() {
        return Err(format!(
            "overlay accept: `only` ({}) names candidate stanzas, and no candidate was given",
            opts.only.join(", ")
        ));
    }
    let surface = if opts.surface.is_empty() {
        "engine"
    } else {
        opts.surface
    };
    let path = Path::new(repo_path).join(OVERLAY_FILE);
    let base_text = read_overlay(&path)?;
    let base_src = base_text.as_deref().unwrap_or(EMPTY_OVERLAY);

    let mut added: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut duplicates = 0;
    let merged_text = match candidate_text {
        Some(text) => {
            let cand = parse_candidate(text).map_err(|e| format!("overlay accept: {e}"))?;
            let only = chosen_refs(&cand, &opts.only)?;
            let merged = merge(base_text.as_deref(), &cand, only.as_deref())
                .map_err(|e| format!("overlay accept: {e}"))?;
            for (r, at) in &merged.refs {
                if *at != Location::AlreadyPresent {
                    *added.entry(r.section).or_default() += 1;
                }
            }
            duplicates = merged.duplicates;
            merged.text
        }
        None => base_src.to_string(),
    };

    let (new_text, removed, removed_kept) = if opts.remove.is_empty() {
        (merged_text, 0, BTreeMap::new())
    } else {
        let identities = removal_identities(repo_path, base_src, &opts.remove, surface)?;
        remove_stanzas(&merged_text, &identities)?
    };
    validate(base_src, &new_text, &added, &removed_kept)?;

    let diff = unified_diff(base_text.as_deref(), &new_text);
    let changed = base_text.as_deref() != Some(new_text.as_str());
    let written = !opts.dry_run && changed;
    if written {
        write_atomically(&path, &new_text)?;
    }
    let n = |s: &str| added.get(s).copied().unwrap_or(0);
    eprintln!(
        "[overlay] accept repo={repo_path} added={} (route_prefix={} wrapper={} edge={} constants={} entrypoints={}) removed={removed} duplicates={duplicates} file={OVERLAY_FILE} dry_run={} surface={surface}",
        added.values().sum::<usize>(),
        n(MARKER_SECTIONS[0]),
        n(MARKER_SECTIONS[1]),
        n(MARKER_SECTIONS[2]),
        n(MARKER_SECTIONS[3]),
        n(MARKER_SECTIONS[4]),
        opts.dry_run,
    );
    Ok(AcceptSummary {
        added,
        removed,
        duplicates,
        file: path.display().to_string(),
        dry_run: opts.dry_run,
        written,
        diff,
    })
}

/// The file's text; `None` when there is none.
fn read_overlay(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| format!("overlay accept: {OVERLAY_FILE} is not valid UTF-8")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("overlay accept: cannot read {OVERLAY_FILE}: {e}")),
    }
}

/// `only`'s displays as the candidate's refs, deduped; `None` for all.
fn chosen_refs(cand: &Candidate, only: &[String]) -> Result<Option<Vec<StanzaRef>>, String> {
    if only.is_empty() {
        return Ok(None);
    }
    let mut refs: Vec<StanzaRef> = Vec::new();
    for name in only {
        let r = cand
            .stanzas()
            .iter()
            .find(|s| s.to_string() == *name)
            .ok_or_else(|| {
                let known: Vec<String> = cand.stanzas().iter().map(StanzaRef::to_string).collect();
                format!(
                    "overlay accept: stanza {name} is not in the candidate (it has: {})",
                    known.join(", ")
                )
            })?;
        if !refs.contains(r) {
            refs.push(r.clone());
        }
    }
    Ok(Some(refs))
}

/// What a rule row's stanza is, independent of where it sits in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Identity {
    /// `[[edge]]`: from, to, category.
    Edge {
        from: String,
        to: String,
        category: String,
    },
    /// `[[constraint]]`, `[[decision]]`, or a `[[note]]` with an id.
    Declared { section: &'static str, id: String },
    /// A `[[note]]` without an id: its anchor and text.
    AnonymousNote { anchor: String, text: String },
    /// An `[entrypoints] qnames` pattern.
    Entrypoint(String),
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Identity::Edge { from, to, category } => {
                write!(f, "[[edge]] {from} -> {to} category={category}")
            }
            Identity::Declared { section, id } => write!(f, "[[{section}]] id={id}"),
            Identity::AnonymousNote { anchor, .. } => write!(f, "[[note]] anchor={anchor}"),
            Identity::Entrypoint(p) => write!(f, "[entrypoints] qname {p}"),
        }
    }
}

impl Identity {
    /// The loader section it belongs to (a `section_counts` name).
    fn section(&self) -> &'static str {
        match self {
            Identity::Edge { .. } => "edge",
            Identity::Declared { section, .. } => section,
            Identity::AnonymousNote { .. } => "note",
            Identity::Entrypoint(_) => "entrypoints",
        }
    }

    /// Does this document table (a `[[section]]` or an inline table of it)
    /// have this identity?
    fn matches(&self, t: &dyn TableLike) -> bool {
        let field = |key: &str| t.get(key).and_then(Item::as_str);
        match self {
            Identity::Edge { from, to, category } => {
                field("from") == Some(from.as_str())
                    && field("to") == Some(to.as_str())
                    && field("category") == Some(category.as_str())
            }
            Identity::Declared { id, .. } => field("id") == Some(id.as_str()),
            Identity::AnonymousNote { anchor, text } => {
                t.get("id").is_none()
                    && field("anchor") == Some(anchor.as_str())
                    && field("text") == Some(text.as_str())
            }
            Identity::Entrypoint(_) => false,
        }
    }

    /// The loader-kept entries of `loaded` with this identity.
    fn kept_in(&self, loaded: &LoadedConfig) -> usize {
        let c = &loaded.config;
        match self {
            Identity::Edge { from, to, category } => c
                .edge
                .iter()
                .map(|s| s.get_ref())
                .filter(|e| e.from == *from && e.to == *to && e.category == *category)
                .count(),
            Identity::Declared { section, id } => match *section {
                "constraint" => c
                    .constraint
                    .iter()
                    .filter(|s| s.get_ref().id == *id)
                    .count(),
                "decision" => c.decision.iter().filter(|s| s.get_ref().id == *id).count(),
                _ => c
                    .note
                    .iter()
                    .filter(|s| s.get_ref().id.as_deref() == Some(id.as_str()))
                    .count(),
            },
            Identity::AnonymousNote { anchor, text } => c
                .note
                .iter()
                .map(|s| s.get_ref())
                .filter(|n| n.id.is_none() && n.anchor == *anchor && n.text == *text)
                .count(),
            Identity::Entrypoint(p) => c
                .entrypoints
                .qnames
                .iter()
                .filter(|q| q.get_ref() == p)
                .count(),
        }
    }
}

/// The identities of the rule rows `ids` name, read off the repo's gaps
/// report and `base_src` (the text that report read); deduped, in `ids`
/// order. Every problem is collected into one `Err`.
fn removal_identities(
    repo_path: &str,
    base_src: &str,
    ids: &[String],
    surface: &'static str,
) -> Result<Vec<Identity>, String> {
    let built = build_quiet(std::slice::from_ref(&repo_path.to_string()))?;
    let gaps_opts = GapsOptions {
        surface,
        ..GapsOptions::default()
    };
    let report = gaps_report(&built.merged, &repo_roots(&built), &gaps_opts)?;
    let loaded = glia_config::parse_str(base_src);
    let mut problems: Vec<String> = Vec::new();
    let mut out: Vec<Identity> = Vec::new();
    for id in ids {
        let Some(row) = report.rows.iter().find(|r| r.id == *id) else {
            problems.push(format!(
                "{id} is not a gap of {repo_path} (no gaps row has it)"
            ));
            continue;
        };
        if row.category != ORPHANED_RULE && row.category != REDUNDANT_RULE {
            problems.push(format!(
                "{id} is a {} row; only {ORPHANED_RULE} / {REDUNDANT_RULE} rows are removed",
                row.category
            ));
            continue;
        }
        match identity_of(row, &loaded) {
            Some(i) if !out.contains(&i) => out.push(i),
            Some(_) => {}
            None => problems.push(format!(
                "{id}: no {} stanza at {OVERLAY_FILE}:{} (the file changed during accept?)",
                row.kind,
                row.line.unwrap_or(0)
            )),
        }
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(format!(
            "overlay accept: {}; nothing written",
            problems.join("; ")
        ))
    }
}

/// A rule row's stanza identity: the stanza of the row's section whose
/// header (an entrypoint: the row's pattern) the report located.
fn identity_of(row: &GapRow, loaded: &LoadedConfig) -> Option<Identity> {
    let line = row.line.and_then(|l| u32::try_from(l).ok())?;
    let c = &loaded.config;
    let at = |span: std::ops::Range<usize>| loaded.line_of(span) == line;
    match row.kind {
        "edge" => c.edge.iter().find(|s| at(s.span())).map(|s| {
            let e = s.get_ref();
            Identity::Edge {
                from: e.from.clone(),
                to: e.to.clone(),
                category: e.category.clone(),
            }
        }),
        "constraint" => c
            .constraint
            .iter()
            .find(|s| at(s.span()))
            .map(|s| Identity::Declared {
                section: "constraint",
                id: s.get_ref().id.clone(),
            }),
        "decision" => c
            .decision
            .iter()
            .find(|s| at(s.span()))
            .map(|s| Identity::Declared {
                section: "decision",
                id: s.get_ref().id.clone(),
            }),
        "note" => c.note.iter().find(|s| at(s.span())).map(|s| {
            let n = s.get_ref();
            match &n.id {
                Some(id) => Identity::Declared {
                    section: "note",
                    id: id.clone(),
                },
                None => Identity::AnonymousNote {
                    anchor: n.anchor.clone(),
                    text: n.text.clone(),
                },
            }
        }),
        "entrypoint" => c
            .entrypoints
            .qnames
            .iter()
            .any(|q| *q.get_ref() == row.qname)
            .then(|| Identity::Entrypoint(row.qname.clone())),
        _ => None,
    }
}

/// `text` with every entry of each identity deleted: the new text, the
/// entries deleted, and per section the loader-kept entries among them.
fn remove_stanzas(
    text: &str,
    identities: &[Identity],
) -> Result<(String, usize, BTreeMap<&'static str, usize>), String> {
    let before = glia_config::parse_str(text);
    let src = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut doc = DocumentMut::from_str(src).map_err(|e| {
        format!(
            "overlay accept: {OVERLAY_FILE} is not valid TOML: {}",
            e.to_string().trim().replace('\n', "; ")
        )
    })?;
    let mut removed = 0;
    let mut kept: BTreeMap<&'static str, usize> = BTreeMap::new();
    for ident in identities {
        let n = match ident {
            Identity::Entrypoint(p) => remove_pattern(&mut doc, p),
            _ => remove_tables(&mut doc, ident),
        };
        if n == 0 {
            return Err(format!(
                "overlay accept: {ident} is not in {OVERLAY_FILE}; nothing written"
            ));
        }
        removed += n;
        *kept.entry(ident.section()).or_default() += ident.kept_in(&before);
    }
    Ok((doc.to_string(), removed, kept))
}

/// Delete every `[[section]]` table (or inline table of a `section = [...]`
/// array) with identity `ident`; a section left empty goes whole.
fn remove_tables(doc: &mut DocumentMut, ident: &Identity) -> usize {
    let section = ident.section();
    let Some(item) = doc.get_mut(section) else {
        return 0;
    };
    let (n, empty) = match item {
        Item::ArrayOfTables(tables) => {
            let len = tables.len();
            tables.retain(|t| !ident.matches(t));
            (len - tables.len(), tables.is_empty())
        }
        Item::Value(Value::Array(items)) => {
            let len = items.len();
            items.retain(|v| !v.as_inline_table().is_some_and(|t| ident.matches(t)));
            (len - items.len(), items.is_empty())
        }
        _ => (0, false),
    };
    if n > 0 && empty {
        doc.remove(section);
    }
    n
}

/// Delete every `[entrypoints] qnames` item equal to `pattern`.
fn remove_pattern(doc: &mut DocumentMut, pattern: &str) -> usize {
    let Some(qnames) = doc
        .get_mut("entrypoints")
        .and_then(Item::as_table_like_mut)
        .and_then(|t| t.get_mut("qnames"))
        .and_then(Item::as_array_mut)
    else {
        return 0;
    };
    let len = qnames.len();
    qnames.retain(|v| v.as_str() != Some(pattern));
    len - qnames.len()
}

/// A loader error without its `.glia/overlay.toml[:<line>]: ` prefix: an
/// edit shifts lines, never an error.
fn unlocated(e: &str) -> &str {
    let Some(rest) = e
        .strip_prefix(OVERLAY_FILE)
        .and_then(|r| r.strip_prefix(':'))
    else {
        return e;
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let rest = &rest[digits..];
    let rest = if digits > 0 {
        rest.strip_prefix(':').unwrap_or(rest)
    } else {
        rest
    };
    rest.trim_start()
}

/// The new text loads with no error `base_src` did not have, and with its
/// section counts plus `added` minus `removed_kept`.
fn validate(
    base_src: &str,
    new_text: &str,
    added: &BTreeMap<&'static str, usize>,
    removed_kept: &BTreeMap<&'static str, usize>,
) -> Result<(), String> {
    let old = glia_config::parse_str(base_src);
    let new = glia_config::parse_str(new_text);
    let mut allowed: Vec<&str> = old.errors.iter().map(|e| unlocated(e)).collect();
    let mut fresh: Vec<&str> = Vec::new();
    for e in &new.errors {
        match allowed.iter().position(|a| *a == unlocated(e)) {
            Some(i) => {
                allowed.swap_remove(i);
            }
            None => fresh.push(e),
        }
    }
    if !fresh.is_empty() {
        return Err(format!(
            "overlay accept: the new {OVERLAY_FILE} would not load as written: {}; nothing written",
            fresh.join("; ")
        ));
    }
    for ((section, before), (_, now)) in old.section_counts().into_iter().zip(new.section_counts())
    {
        let get = |m: &BTreeMap<&'static str, usize>| m.get(section).copied().unwrap_or(0);
        let want = (before + get(added)).saturating_sub(get(removed_kept));
        if now != want {
            return Err(format!(
                "overlay accept: the new {OVERLAY_FILE} loads {section}={now}, expected {want}; nothing written"
            ));
        }
    }
    Ok(())
}

/// `text` to `path` through `<path>.<pid>.tmp` and a rename; the directory is
/// created when absent.
fn write_atomically(path: &Path, text: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("overlay accept: {} has no parent dir", path.display()))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("overlay accept: cannot create {}: {e}", dir.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!("{name}.{}.tmp", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, text) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "overlay accept: cannot write {}: {e}",
            tmp.display()
        ));
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("overlay accept: cannot replace {}: {e}", path.display())
    })
}

/// One line of a diff: unchanged, removed or added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op<'t> {
    Keep(&'t str),
    Del(&'t str),
    Add(&'t str),
}

/// The line edits turning `a` into `b`: the common head and tail kept, the
/// middle by longest common subsequence (all removed then all added past
/// [`MAX_LCS_CELLS`]).
fn line_ops<'t>(a: &[&'t str], b: &[&'t str]) -> Vec<Op<'t>> {
    let head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let tail = a[head..]
        .iter()
        .rev()
        .zip(b[head..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (ma, mb) = (&a[head..a.len() - tail], &b[head..b.len() - tail]);
    let mut ops: Vec<Op<'t>> = a[..head].iter().copied().map(Op::Keep).collect();
    if ma.len().saturating_mul(mb.len()) > MAX_LCS_CELLS {
        ops.extend(ma.iter().copied().map(Op::Del));
        ops.extend(mb.iter().copied().map(Op::Add));
    } else {
        // lcs[i][j]: the LCS length of ma[i..] and mb[j..].
        let w = mb.len() + 1;
        let mut lcs = vec![0u32; (ma.len() + 1) * w];
        for i in (0..ma.len()).rev() {
            for j in (0..mb.len()).rev() {
                lcs[i * w + j] = if ma[i] == mb[j] {
                    lcs[(i + 1) * w + j + 1] + 1
                } else {
                    lcs[(i + 1) * w + j].max(lcs[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < ma.len() || j < mb.len() {
            if i < ma.len() && j < mb.len() && ma[i] == mb[j] {
                ops.push(Op::Keep(ma[i]));
                i += 1;
                j += 1;
            } else if j == mb.len() || (i < ma.len() && lcs[(i + 1) * w + j] >= lcs[i * w + j + 1])
            {
                ops.push(Op::Del(ma[i]));
                i += 1;
            } else {
                ops.push(Op::Add(mb[j]));
                j += 1;
            }
        }
    }
    ops.extend(a[a.len() - tail..].iter().copied().map(Op::Keep));
    ops
}

/// Unified diff of the file `old` (`None`: no file) -> `new`, with
/// [`DIFF_CONTEXT`] lines of context, cut at [`MAX_DIFF_LINES`] lines; empty
/// when the lines are equal.
fn unified_diff(old: Option<&str>, new: &str) -> String {
    let a: Vec<&str> = old.unwrap_or_default().lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let ops = line_ops(&a, &b);
    let changes: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| !matches!(op, Op::Keep(_)))
        .map(|(i, _)| i)
        .collect();
    let Some(&first) = changes.first() else {
        return String::new();
    };
    // Old / new lines before each op.
    let mut before = Vec::with_capacity(ops.len() + 1);
    let (mut oa, mut ob) = (0usize, 0usize);
    for op in &ops {
        before.push((oa, ob));
        match op {
            Op::Keep(_) => {
                oa += 1;
                ob += 1;
            }
            Op::Del(_) => oa += 1,
            Op::Add(_) => ob += 1,
        }
    }
    // Hunks: change runs at most two contexts of unchanged lines apart merge.
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    let (mut start, mut end) = (first, first);
    for &c in &changes[1..] {
        if c - end - 1 > 2 * DIFF_CONTEXT {
            hunks.push((start, end));
            start = c;
        }
        end = c;
    }
    hunks.push((start, end));

    let mut out: Vec<String> = vec![
        match old {
            Some(_) => format!("--- a/{OVERLAY_FILE}"),
            None => "--- /dev/null".to_string(),
        },
        format!("+++ b/{OVERLAY_FILE}"),
    ];
    for (s, e) in hunks {
        let lo = s.saturating_sub(DIFF_CONTEXT);
        let hi = (e + DIFF_CONTEXT + 1).min(ops.len());
        let span = &ops[lo..hi];
        let olen = span.iter().filter(|o| !matches!(o, Op::Add(_))).count();
        let nlen = span.iter().filter(|o| !matches!(o, Op::Del(_))).count();
        let (oa, ob) = before[lo];
        let ostart = if olen == 0 { oa } else { oa + 1 };
        let nstart = if nlen == 0 { ob } else { ob + 1 };
        out.push(format!("@@ -{ostart},{olen} +{nstart},{nlen} @@"));
        for op in span {
            out.push(match op {
                Op::Keep(l) => format!(" {l}"),
                Op::Del(l) => format!("-{l}"),
                Op::Add(l) => format!("+{l}"),
            });
        }
    }
    if out.len() > MAX_DIFF_LINES {
        let more = out.len() - (MAX_DIFF_LINES - 1);
        out.truncate(MAX_DIFF_LINES - 1);
        out.push(format!(
            "... diff cut at {MAX_DIFF_LINES} lines ({more} more)"
        ));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_of_a_new_file_adds_every_line() {
        assert_eq!(
            unified_diff(None, "version = 1\n\n[[edge]]\n"),
            "--- /dev/null\n+++ b/.glia/overlay.toml\n@@ -0,0 +1,3 @@\n+version = 1\n+\n+[[edge]]\n"
        );
        assert_eq!(unified_diff(Some("a\nb\n"), "a\nb\n"), "");
    }

    #[test]
    fn diff_hunks_carry_context_and_line_numbers() {
        let old: String = (1..=20).map(|i| format!("l{i}\n")).collect();
        let new = old.replace("l2\n", "").replace("l18\n", "l18\nnew\n");
        let d = unified_diff(Some(&old), &new);
        let want = "--- a/.glia/overlay.toml\n+++ b/.glia/overlay.toml\n\
@@ -1,5 +1,4 @@\n l1\n-l2\n l3\n l4\n l5\n\
@@ -16,5 +15,6 @@\n l16\n l17\n l18\n+new\n l19\n l20\n";
        assert_eq!(d, want);
    }

    #[test]
    fn diff_is_cut() {
        let new: String = (0..1000).map(|i| format!("x{i}\n")).collect();
        let d = unified_diff(None, &new);
        let lines: Vec<&str> = d.lines().collect();
        assert_eq!(lines.len(), MAX_DIFF_LINES);
        assert!(lines[MAX_DIFF_LINES - 1].starts_with("... diff cut at 400 lines"));
    }

    #[test]
    fn lcs_keeps_the_common_lines() {
        let a = ["x", "a", "b", "c", "y"];
        let b = ["x", "b", "a", "c", "y"];
        let ops = line_ops(&a, &b);
        let kept = ops.iter().filter(|o| matches!(o, Op::Keep(_))).count();
        assert_eq!(kept, 4, "{ops:?}");
    }

    #[test]
    fn unlocated_strips_the_file_and_line() {
        assert_eq!(
            unlocated(".glia/overlay.toml:12: [[edge]] bad"),
            "[[edge]] bad"
        );
        assert_eq!(unlocated(".glia/overlay.toml: whole file"), "whole file");
        assert_eq!(unlocated("other"), "other");
    }

    #[test]
    fn removal_by_identity_keeps_the_rest() {
        let text = "version = 1\n\n# keep\n[[edge]]\nfrom = \"a\"\nto = \"b\"\ncategory = \"CALLS\"\n\n# drop\n[[edge]]\nfrom = \"x\"\nto = \"y\"\ncategory = \"CALLS\"\n\n[entrypoints]\nqnames = [\"a\", \"gone::*\"]\n\n[[note]]\nanchor = \"q\"\ntext = \"t\"\n";
        let ids = [
            Identity::Edge {
                from: "x".into(),
                to: "y".into(),
                category: "CALLS".into(),
            },
            Identity::Entrypoint("gone::*".into()),
            Identity::AnonymousNote {
                anchor: "q".into(),
                text: "t".into(),
            },
        ];
        let (out, n, kept) = remove_stanzas(text, &ids).expect("removed");
        assert_eq!(n, 3);
        assert_eq!(
            kept,
            BTreeMap::from([("edge", 1), ("entrypoints", 1), ("note", 1)])
        );
        assert_eq!(
            out,
            "version = 1\n\n# keep\n[[edge]]\nfrom = \"a\"\nto = \"b\"\ncategory = \"CALLS\"\n\n[entrypoints]\nqnames = [\"a\"]\n"
        );
        let missing = [Identity::Declared {
            section: "constraint",
            id: "c1".into(),
        }];
        assert!(remove_stanzas(text, &missing).is_err());
    }
}
