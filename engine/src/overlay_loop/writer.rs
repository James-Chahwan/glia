//! Candidate overlays and the format-preserving `.glia/overlay.toml` writer
//! (CE.3b, + DB.9a: rules through the loop).
//!
//! A CANDIDATE is a TOML text holding only overlay sections, entrypoints and
//! rules - `[constants]`, `[entrypoints] qnames`, `[[route_prefix]]`,
//! `[[wrapper]]`, `[[edge]]`, `[[constraint]]` - and an optional `version`.
//! Each entry is one stanza, named by a [`StanzaRef`] (`edge#1`,
//! `constants.GATEWAY`, `entrypoints#2`, `constraint#1`). A stanza names the
//! gap(s) it targets (`glia gaps` row ids, CE.3a) with comment lines
//! `# gap: gap:<16 hex>` directly above its table header, above its key (a
//! constant), or above its item in a multi-line `qnames` array (a pattern; the
//! `[entrypoints]` header's links apply to every pattern). A link is a
//! comment, invisible to the loader, so a candidate stays valid overlay text
//! even pasted in by hand: a candidate-only KEY would default the whole file
//! (`glia_config`: every struct is `deny_unknown_fields`).
//!
//! RULES (DB.9a, 0.5.2 Q9). A `[[constraint]]` stanza is a `forbid_edge`
//! (with DB.1's `transitive` / `via`), a `no_cycle`, or an `invariant` with a
//! `rule`, each with an optional `min_tier`; it is keyed by its `id`. Two
//! kinds stay hand-written and are refused here: `allow` (a waiver of a
//! reflexion divergence is a person's call) and an invariant without `rule`
//! (`glia check` never evaluates prose, so `try` cannot judge it). A
//! version-less candidate is checked as `version = LATEST_VERSION` (2), so a
//! bare rule candidate validates. [`merge`] starts a new file from
//! `version = 2` only when a written rule sets one of
//! [`V2_CONSTRAINT_FIELDS`], and raises an existing `version = 1` to 2 in
//! place (its comment kept) in that case only, refusing the raise when it
//! would also load a stanza the base's version drops. An id the base already
//! declares is a duplicate when the bodies are equal by value, and refused
//! otherwise: a candidate adds rules, never edits one.
//! [`Candidate::unreadable_rules`] is `accept`'s guard against a rule
//! glia check cannot read (it does not validate, or defines no violation; a
//! rule whose scope holds no node is not refused, DT.7c); parsing and
//! merging never call it, so `try` still judges such a stanza.
//!
//! [`parse_candidate`] accepts a candidate only when the loader
//! (`glia_config::parse_str`) reports no error on it: it is all-valid or
//! refused. [`merge`] writes chosen stanzas into the user's file through a
//! `toml_edit` document, so every comment and layout of the file survives: an
//! array stanza lands after the last table of its section (or at the end), a
//! constant joins `[constants]`, a pattern joins `qnames`; each carries its
//! `# gap:` lines as a provenance note. [`without`] takes one merged stanza
//! back out. Both re-check their output with the loader, so the writer never
//! produces a file the loader would drop. `[walk]` / `[[project]]` are never
//! in a candidate: the walk reads them from the file on disk even when a build
//! is given the overlay as text (`BuildOptions::overlay_text`). Nor are the
//! reflexion model's `[[component]]` / `[[layer]]` or the prose `[[decision]]`
//! / `[[note]]`: they are a person's declarations.
//!
//! Marker, printed by [`report_candidate`] (the caller that names the file):
//! `[overlay] candidate file=<path or -> stanzas=<n> (route_prefix=<a> wrapper=<b> edge=<c> constants=<d> entrypoints=<e> constraint=<k>) gap_links=<g> errors=<x>`.

use std::fmt;
use std::str::FromStr;

use glia_code_domain::glia_config::{
    self, LATEST_VERSION, LoadedConfig, OVERLAY_FILE, V2_CONSTRAINT_FIELDS,
};
use glia_rules::parser::parse_program;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, TableLike, Value};

/// The sections a candidate may carry, in marker order (`constraint` last,
/// DB.9a, so every earlier marker field keeps its place).
const CANDIDATE_SECTIONS: [&str; 6] = [
    "route_prefix",
    "wrapper",
    "edge",
    "constants",
    "entrypoints",
    "constraint",
];
/// The candidate sections written as `[[section]]` tables.
const TABLE_SECTIONS: [&str; 4] = ["route_prefix", "wrapper", "edge", "constraint"];
/// The rule output `glia check` evaluates (check.rs `RULE_OUTPUT`): a rule
/// that defines no `violation` rule has nothing for check to report.
const RULE_OUTPUT: &str = "violation";
/// The version a file needs for a `[[constraint]]` that sets one of
/// [`V2_CONSTRAINT_FIELDS`] (DB.1).
const RULE_VERSION: i64 = LATEST_VERSION as i64;
/// The comment that links a stanza to the gaps it targets.
const GAP_LINK: &str = "gap:";
/// Indent of a `qnames` item the writer puts on its own line.
const ITEM_INDENT: &str = "    ";

/// One stanza of a candidate. Displays as the handle `try` / `accept` / the
/// CLI name it by: `wrapper#2`, `edge#1`, `route_prefix#1`,
/// `constants.GATEWAY`, `entrypoints#3`, `constraint#1`.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StanzaRef {
    /// `route_prefix`, `wrapper`, `edge`, `constants`, `entrypoints` or
    /// `constraint`.
    pub section: &'static str,
    /// 1-based, within its section of the candidate, in candidate order.
    pub index: usize,
    /// The constant's name (`constants`), the pattern (`entrypoints`) or the
    /// rule's `id` (`constraint`; its handle stays `constraint#<n>`, the table
    /// convention: an id may hold any non-control char, a handle must survive
    /// a shell).
    pub key: Option<String>,
    /// The `gap:<16 hex>` ids its `# gap:` comments link, in order, deduped.
    pub gaps: Vec<String>,
}

impl fmt::Display for StanzaRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.section, &self.key) {
            ("constants", Some(name)) => write!(f, "constants.{name}"),
            (section, _) => write!(f, "{section}#{}", self.index),
        }
    }
}

impl StanzaRef {
    /// Same stanza of the same candidate (the gaps are not compared).
    fn names(&self, other: &StanzaRef) -> bool {
        self.section == other.section && self.index == other.index
    }
}

/// A parsed, loader-valid candidate: its stanzas in candidate order, and the
/// document [`merge`] copies them from.
#[derive(Clone, Debug)]
pub struct Candidate {
    doc: DocumentMut,
    stanzas: Vec<StanzaRef>,
}

impl Candidate {
    /// Every stanza, in candidate order.
    pub fn stanzas(&self) -> &[StanzaRef] {
        &self.stanzas
    }

    /// The `[overlay] candidate` marker of this (valid) candidate.
    pub fn marker(&self, file: Option<&str>) -> String {
        marker(file, &self.stanzas, 0)
    }

    /// `accept`'s rule guard (DB.9a): per chosen `[[constraint]]` stanza (all,
    /// or the ones `only` names) whose `rule` glia check cannot read (it does
    /// not validate, or defines no violation; a rule whose scope holds no node
    /// is not refused, DT.7c: it parses, and glia check lists it as one that
    /// checks nothing), one refusal line - a rule that does not validate
    /// against the code schema and prelude ([`crate::rules::validate`], no
    /// graph; the reader DB.4's `rotted_rule` uses), or one that defines no
    /// `violation` rule.
    /// Empty when every chosen rule is readable. [`parse_candidate`] and
    /// [`merge`] never call it: `try` builds such a stanza and judges it.
    pub(crate) fn unreadable_rules(&self, only: Option<&[StanzaRef]>) -> Vec<String> {
        let mut out = Vec::new();
        for r in self.stanzas.iter().filter(|s| {
            s.section == "constraint" && only.is_none_or(|o| o.iter().any(|x| x.names(s)))
        }) {
            let Some(rule) = cand_table(&self.doc, r)
                .and_then(|t| t.get("rule"))
                .and_then(Item::as_str)
            else {
                continue;
            };
            let id = r.key.as_deref().unwrap_or_default();
            let diags = crate::rules::validate(rule);
            if let Some(first) = diags.first() {
                let mut line = format!(
                    "candidate: {r}: rule {id:?} does not validate at {}:{}: {}",
                    first.line, first.column, first.message
                );
                if diags.len() > 1 {
                    line.push_str(&format!(" (+{} more)", diags.len() - 1));
                }
                out.push(line);
            } else if !parse_program(id, rule)
                .is_ok_and(|p| p.rules.iter().any(|x| x.head.pred == RULE_OUTPUT))
            {
                out.push(format!(
                    "candidate: {r}: rule {id:?} defines no `{RULE_OUTPUT}` predicate, so `glia check` cannot evaluate it"
                ));
            }
        }
        out
    }
}

/// Where a merged stanza landed in [`Merged::text`], so [`without`] removes
/// exactly it.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    /// `[[section]]` table `index` (0-based, within its section).
    Table { section: &'static str, index: usize },
    /// The `[constants]` key `name`.
    Constant { name: String },
    /// Item `index` (0-based) of `[entrypoints] qnames`, holding `pattern`.
    Entrypoint { index: usize, pattern: String },
    /// Already in the base (the same constant value, the same pattern, or a
    /// `[[constraint]]` of the same id and body): nothing was written, so
    /// taking it out changes nothing.
    AlreadyPresent,
}

/// The base text with the chosen stanzas merged in.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merged {
    /// The merged `.glia/overlay.toml` text.
    pub text: String,
    /// Each chosen stanza, in candidate order, and where it landed.
    pub refs: Vec<(StanzaRef, Location)>,
    /// Chosen stanzas already in the base ([`Location::AlreadyPresent`]).
    pub duplicates: usize,
}

impl Merged {
    /// [`Merged::text`] without the merged stanza `r` (see [`without`]).
    pub fn without(&self, r: &StanzaRef) -> Result<String, String> {
        let (_, at) = self
            .refs
            .iter()
            .find(|(s, _)| s.names(r))
            .ok_or_else(|| format!("stanza {r} is not in this merge"))?;
        without(&self.text, at)
    }
}

/// Parse a candidate: every stanza, its gap links, and the loader's verdict.
/// `Err` lists every problem (a section a candidate may not carry, a gap link
/// that is not an id, each loader error at its candidate line) - a candidate
/// is all-valid or refused.
pub fn parse_candidate(text: &str) -> Result<Candidate, String> {
    scan(text).into_result()
}

/// [`parse_candidate`], printing the `[overlay] candidate` marker for `file`
/// (`-` when the text did not come from a file), refused or not.
pub fn report_candidate(file: Option<&str>, text: &str) -> Result<Candidate, String> {
    let scanned = scan(text);
    eprintln!("{}", marker(file, &scanned.stanzas, scanned.errors.len()));
    scanned.into_result()
}

/// Merge `cand`'s stanzas (all, or the ones `only` names) into `base_text`
/// (the user's `.glia/overlay.toml`; `None` when there is none, which starts
/// the file from `version = 1`, or `version = 2` when a chosen
/// `[[constraint]]` sets one of [`V2_CONSTRAINT_FIELDS`]). Untouched text is
/// kept byte for byte; in that rule case only, an existing integer `version`
/// below 2 is raised to 2 in place, its comment kept (never added, never
/// lowered). A constant pinned in the base to another value, or a
/// `[[constraint]]` id the base declares with another body, is an `Err` (the
/// same value, a pattern already listed, or the same rule, is a duplicate:
/// nothing written). A raise that would also load a stanza the base's version
/// drops is an `Err`. The merged text must load with exactly the (raised)
/// base's loader errors and with its section counts plus the stanzas written,
/// else `Err` and nothing merged.
pub fn merge(
    base_text: Option<&str>,
    cand: &Candidate,
    only: Option<&[StanzaRef]>,
) -> Result<Merged, String> {
    if let Some(only) = only
        && let Some(r) = only
            .iter()
            .find(|r| !cand.stanzas.iter().any(|s| s.names(r)))
    {
        let known: Vec<String> = cand.stanzas.iter().map(StanzaRef::to_string).collect();
        return Err(format!(
            "stanza {r} is not in the candidate (it has: {})",
            known.join(", ")
        ));
    }
    let chosen: Vec<&StanzaRef> = cand
        .stanzas
        .iter()
        .filter(|s| only.is_none_or(|o| o.iter().any(|r| r.names(s))))
        .collect();
    let needs_v2 = chosen.iter().any(|r| {
        r.section == "constraint"
            && cand_table(&cand.doc, r)
                .is_some_and(|t| t.iter().any(|(k, _)| V2_CONSTRAINT_FIELDS.contains(&k)))
    });
    let fresh = format!("version = {}\n", if needs_v2 { RULE_VERSION } else { 1 });
    let base_src = base_text.map_or(fresh.as_str(), strip_bom);
    let mut doc = DocumentMut::from_str(base_src)
        .map_err(|e| format!("{OVERLAY_FILE} is not valid TOML: {}", flat(&e.to_string())))?;
    // The base the merged text is judged against: the base itself, or the
    // base with its version raised (refused when the raise alone would load
    // more than the base does).
    let mut base = glia_config::parse_str(base_src);
    if needs_v2 && base_text.is_some() {
        let mut raised = doc.clone();
        if let Some(old) = require_version(&mut raised, RULE_VERSION) {
            let raised_cfg = glia_config::parse_str(&raised.to_string());
            hidden_stanzas(&base, &raised_cfg, old)?;
            require_version(&mut doc, RULE_VERSION);
            base = raised_cfg;
        }
    }
    let mut next_position = max_position(doc.as_table()) + 1;
    let mut refs = Vec::new();
    let mut duplicates = 0;
    let mut added = [0usize; CANDIDATE_SECTIONS.len()];
    for r in chosen {
        let at = match r.section {
            "constants" => place_constant(&mut doc, &cand.doc, r, &mut next_position)?,
            "entrypoints" => place_pattern(&mut doc, r, &mut next_position)?,
            "constraint" => place_constraint(&mut doc, &cand.doc, r, &mut next_position)?,
            _ => place_table(&mut doc, &cand.doc, r, &mut next_position)?,
        };
        if at == Location::AlreadyPresent {
            duplicates += 1;
        } else if let Some(i) = CANDIDATE_SECTIONS.iter().position(|s| *s == r.section) {
            added[i] += 1;
        }
        refs.push((r.clone(), at));
    }
    let text = doc.to_string();
    let after = glia_config::parse_str(&text);
    let (mut want, mut got) = (unlocated(&base), unlocated(&after));
    want.sort();
    got.sort();
    if want != got {
        let new: Vec<&str> = got.into_iter().filter(|e| !want.contains(e)).collect();
        return Err(format!(
            "the merged {OVERLAY_FILE} would not load as written: {}",
            new.join("; ")
        ));
    }
    for ((section, before), (_, now)) in base
        .section_counts()
        .into_iter()
        .zip(after.section_counts())
    {
        let plus = CANDIDATE_SECTIONS
            .iter()
            .position(|s| *s == section)
            .map_or(0, |i| added[i]);
        if now != before + plus {
            let why = if base.errors.is_empty() {
                String::new()
            } else {
                format!(" (the base file: {})", base.errors.join("; "))
            };
            return Err(format!(
                "the merged {OVERLAY_FILE} loads {section}={now}, expected {}{why}",
                before + plus
            ));
        }
    }
    Ok(Merged {
        text,
        refs,
        duplicates,
    })
}

/// `merged_text` (a [`Merged::text`]) without the stanza at `at`: that table,
/// constant key or `qnames` item is removed and everything else kept byte for
/// byte. `at` must come from the merge that made the text; a location the
/// text does not hold is an `Err`, never a guess.
pub fn without(merged_text: &str, at: &Location) -> Result<String, String> {
    if *at == Location::AlreadyPresent {
        return Ok(merged_text.to_string());
    }
    let mut doc = DocumentMut::from_str(strip_bom(merged_text)).map_err(|e| {
        format!(
            "the merged text is not valid TOML: {}",
            flat(&e.to_string())
        )
    })?;
    match at {
        Location::Table { section, index } => {
            let tables = doc
                .get_mut(section)
                .and_then(Item::as_array_of_tables_mut)
                .filter(|a| *index < a.len())
                .ok_or_else(|| format!("the merged text has no [[{section}]] #{}", index + 1))?;
            tables.remove(*index);
        }
        Location::Constant { name } => {
            doc.get_mut("constants")
                .and_then(Item::as_table_mut)
                .and_then(|t| t.remove(name))
                .ok_or_else(|| format!("the merged text pins no constant {name}"))?;
        }
        Location::Entrypoint { index, pattern } => {
            let qnames = doc
                .get_mut("entrypoints")
                .and_then(Item::as_table_mut)
                .and_then(|t| t.get_mut("qnames"))
                .and_then(Item::as_array_mut)
                .filter(|a| a.get(*index).and_then(Value::as_str) == Some(pattern.as_str()))
                .ok_or_else(|| {
                    format!(
                        "the merged text has no entrypoints pattern {pattern:?} at #{}",
                        index + 1
                    )
                })?;
            qnames.remove(*index);
        }
        Location::AlreadyPresent => {}
    }
    Ok(doc.to_string())
}

/// What [`scan`] found: the stanzas, in candidate order, and every problem.
struct Scanned {
    doc: Option<DocumentMut>,
    stanzas: Vec<StanzaRef>,
    errors: Vec<String>,
}

impl Scanned {
    fn into_result(self) -> Result<Candidate, String> {
        match self.doc {
            Some(doc) if self.errors.is_empty() => Ok(Candidate {
                doc,
                stanzas: self.stanzas,
            }),
            _ => Err(self.errors.join("; ")),
        }
    }
}

fn scan(text: &str) -> Scanned {
    let text = strip_bom(text);
    let doc = match DocumentMut::from_str(text) {
        Ok(doc) => doc,
        Err(e) => {
            let errors = vec![format!(
                "candidate: not valid TOML: {}",
                flat(&e.to_string())
            )];
            return Scanned {
                doc: None,
                stanzas: Vec::new(),
                errors,
            };
        }
    };
    let mut errors = Vec::new();
    // (document position, stanza): sorted by position below, so stanzas come
    // out in candidate order across sections.
    let mut found: Vec<(usize, StanzaRef)> = Vec::new();
    for (key, item) in doc.iter() {
        match (key, item) {
            ("version", _) => {}
            (section, Item::ArrayOfTables(tables)) if TABLE_SECTIONS.contains(&section) => {
                let section = TABLE_SECTIONS
                    .iter()
                    .find(|s| **s == section)
                    .copied()
                    .unwrap_or_default();
                for (i, t) in tables.iter().enumerate() {
                    let key = (section == "constraint")
                        .then(|| t.get("id").and_then(Item::as_str).map(str::to_string))
                        .flatten();
                    let mut r = stanza(section, i + 1, key);
                    r.gaps =
                        gap_links(t.decor().prefix().and_then(|p| p.as_str()), &r, &mut errors);
                    if section == "constraint" {
                        loop_only_refusals(t, &r, &mut errors);
                    }
                    found.push((t.position().unwrap_or(0), r));
                }
            }
            ("constants", Item::Table(t)) => {
                for (i, (name, _)) in t.iter().enumerate() {
                    let mut r = stanza("constants", i + 1, Some(name.to_string()));
                    let decor = t
                        .key(name)
                        .and_then(|k| k.leaf_decor().prefix())
                        .and_then(|p| p.as_str());
                    r.gaps = gap_links(decor, &r, &mut errors);
                    found.push((t.position().unwrap_or(0), r));
                }
            }
            ("entrypoints", Item::Table(t)) => {
                let header = t.decor().prefix().and_then(|p| p.as_str());
                let patterns = t.get("qnames").and_then(Item::as_array);
                for (i, v) in patterns.into_iter().flat_map(Array::iter).enumerate() {
                    let mut r = stanza("entrypoints", i + 1, v.as_str().map(str::to_string));
                    let mut gaps = gap_links(header, &r, &mut errors);
                    for g in gap_links(v.decor().prefix().and_then(|p| p.as_str()), &r, &mut errors)
                    {
                        if !gaps.contains(&g) {
                            gaps.push(g);
                        }
                    }
                    r.gaps = gaps;
                    found.push((t.position().unwrap_or(0), r));
                }
            }
            (section, item) if CANDIDATE_SECTIONS.contains(&section) => {
                let form = if TABLE_SECTIONS.contains(&section) {
                    "[[{s}]] tables"
                } else {
                    "a [{s}] table"
                };
                errors.push(format!(
                    "candidate: `{section}` is written as {}; write it as {}",
                    item.type_name(),
                    form.replace("{s}", section)
                ));
            }
            (section, item) => {
                let header = if item.is_array_of_tables() {
                    format!("[[{section}]]")
                } else {
                    format!("[{section}]")
                };
                errors.push(format!(
                    "candidate: {header}: a candidate carries only overlay sections, entrypoints and rules ({})",
                    CANDIDATE_SECTIONS.join(", ")
                ));
            }
        }
    }
    found.sort_by_key(|(position, _)| *position);
    let stanzas: Vec<StanzaRef> = found.into_iter().map(|(_, r)| r).collect();
    // The loader's verdict, at the candidate's own lines. A text with no
    // `version` gets `version = LATEST_VERSION` prepended, one line: version 2
    // loads every version-1 text exactly as before (DB.1), and a bare rule
    // candidate validates.
    let (checked, shift) = if doc.contains_key("version") {
        (text.to_string(), 0)
    } else {
        (format!("version = {LATEST_VERSION}\n{text}"), 1)
    };
    for e in glia_config::parse_str(&checked).errors {
        let (line, msg) = split_location(&e);
        errors.push(match line {
            Some(l) => format!("candidate:{}: {msg}", l.saturating_sub(shift)),
            None => format!("candidate: {msg}"),
        });
    }
    Scanned {
        doc: Some(doc),
        stanzas,
        errors,
    }
}

fn stanza(section: &'static str, index: usize, key: Option<String>) -> StanzaRef {
    StanzaRef {
        section,
        index,
        key,
        gaps: Vec::new(),
    }
}

/// The gap ids a decor's `# gap:` comment lines link; a link that is not
/// `gap:<16 lower hex>` is an error naming the stanza.
fn gap_links(decor: Option<&str>, r: &StanzaRef, errors: &mut Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in decor.unwrap_or_default().lines() {
        let Some(comment) = line.trim().strip_prefix('#') else {
            continue;
        };
        let Some(links) = comment.trim_start().strip_prefix(GAP_LINK) else {
            continue;
        };
        let ids: Vec<&str> = links
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .collect();
        if ids.is_empty() {
            errors.push(format!(
                "candidate: {r}: a `# gap:` comment links no gap id"
            ));
        }
        for id in ids {
            if !is_gap_id(id) {
                errors.push(format!(
                    "candidate: {r}: `# gap:` link {id:?} is not gap:<16 lower hex>"
                ));
            } else if !out.iter().any(|g| g == id) {
                out.push(id.to_string());
            }
        }
    }
    out
}

/// `gap:` + 16 lower-hex: a `glia gaps` row id (CE.3a).
fn is_gap_id(id: &str) -> bool {
    id.strip_prefix(GAP_LINK).is_some_and(|h| {
        h.len() == 16
            && h.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// The two `[[constraint]]` refusals the loader does not make (DB.9a): both
/// kinds load from a hand-written file, and neither goes through the loop.
fn loop_only_refusals(t: &Table, r: &StanzaRef, errors: &mut Vec<String>) {
    match t.get("kind").and_then(Item::as_str) {
        Some("allow") => errors.push(format!(
            "candidate: {r}: [[constraint]] kind allow waives a reflexion divergence; a person declares it by hand, never through the loop"
        )),
        Some("invariant") if !t.contains_key("rule") => {
            let id = r.key.as_deref().unwrap_or_default();
            errors.push(format!(
                "candidate: {r}: [[constraint]] invariant {id:?} has no `rule`: `glia check` never evaluates a prose invariant, so `try` cannot judge it; a person declares it by hand"
            ));
        }
        _ => {}
    }
}

fn marker(file: Option<&str>, stanzas: &[StanzaRef], errors: usize) -> String {
    let n = |section: &str| stanzas.iter().filter(|r| r.section == section).count();
    format!(
        "[overlay] candidate file={} stanzas={} (route_prefix={} wrapper={} edge={} constants={} entrypoints={} constraint={}) gap_links={} errors={errors}",
        file.unwrap_or("-"),
        stanzas.len(),
        n("route_prefix"),
        n("wrapper"),
        n("edge"),
        n("constants"),
        n("entrypoints"),
        n("constraint"),
        stanzas.iter().map(|r| r.gaps.len()).sum::<usize>(),
    )
}

/// A loader error's `.glia/overlay.toml[:<line>]: ` prefix, split off.
fn split_location(e: &str) -> (Option<usize>, &str) {
    let Some(rest) = e
        .strip_prefix(OVERLAY_FILE)
        .and_then(|r| r.strip_prefix(':'))
    else {
        return (None, e);
    };
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let line = rest[..digits].parse().ok();
    let rest = &rest[digits..];
    let rest = if digits > 0 {
        rest.strip_prefix(':').unwrap_or(rest)
    } else {
        rest
    };
    (line, rest.trim_start())
}

/// A config's loader errors without their line: a merge shifts lines, never
/// an error.
fn unlocated(l: &LoadedConfig) -> Vec<&str> {
    l.errors.iter().map(|e| split_location(e).1).collect()
}

fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

fn flat(s: &str) -> String {
    s.trim().replace('\n', "; ")
}

/// The largest document position of any table under `t` (0 when none).
fn max_position(t: &Table) -> usize {
    let mut max = t.position().unwrap_or(0);
    for (_, item) in t.iter() {
        match item {
            Item::Table(sub) => max = max.max(max_position(sub)),
            Item::ArrayOfTables(tables) => {
                for sub in tables.iter() {
                    max = max.max(max_position(sub));
                }
            }
            _ => {}
        }
    }
    max
}

/// A decor prefix without its leading blank lines: the comment block right
/// above a header or key.
fn comment_block(prefix: Option<&str>) -> &str {
    let mut rest = prefix.unwrap_or_default();
    while let Some(end) = rest.find('\n') {
        if !rest[..end].trim().is_empty() {
            break;
        }
        rest = &rest[end + 1..];
    }
    rest
}

/// Push a copy of the candidate's `[[section]]` table after the last table of
/// that section in `doc` (a new section goes at the end), with its comment
/// block (its `# gap:` lines) under one blank line.
fn place_table(
    doc: &mut DocumentMut,
    cand: &DocumentMut,
    r: &StanzaRef,
    next: &mut usize,
) -> Result<Location, String> {
    let src = cand_table(cand, r).ok_or_else(|| format!("stanza {r} is not in the candidate"))?;
    let mut table = src.clone();
    let prefix = format!(
        "\n{}",
        comment_block(src.decor().prefix().and_then(|p| p.as_str()))
    );
    table.decor_mut().set_prefix(prefix);
    if !doc.contains_key(r.section) {
        doc.insert(r.section, Item::ArrayOfTables(ArrayOfTables::new()));
    }
    let tables = doc
        .get_mut(r.section)
        .and_then(Item::as_array_of_tables_mut)
        .ok_or_else(|| {
            format!(
                "{OVERLAY_FILE} writes `{}` as something other than [[{}]] tables; merge by hand",
                r.section, r.section
            )
        })?;
    // Tables render in position order and a tie keeps document order, so the
    // last table's position puts the new one right after it.
    let position = match tables.iter().last().and_then(Table::position) {
        Some(p) => p,
        None => bump(next),
    };
    table.set_position(position);
    tables.push(table);
    Ok(Location::Table {
        section: r.section,
        index: tables.len() - 1,
    })
}

/// The candidate's `[[section]]` table `r` names.
fn cand_table<'c>(cand: &'c DocumentMut, r: &StanzaRef) -> Option<&'c Table> {
    let i = r.index.checked_sub(1)?;
    cand.get(r.section)
        .and_then(Item::as_array_of_tables)
        .and_then(|a| a.get(i))
}

/// Place the candidate's `[[constraint]]` `r` (DB.9a): when `doc` already
/// declares its id, the same body (by value) is a duplicate and another body
/// an `Err` - a candidate adds rules and never edits one; otherwise
/// [`place_table`].
fn place_constraint(
    doc: &mut DocumentMut,
    cand: &DocumentMut,
    r: &StanzaRef,
    next: &mut usize,
) -> Result<Location, String> {
    let src = cand_table(cand, r).ok_or_else(|| format!("stanza {r} is not in the candidate"))?;
    if let Some(id) = r.key.as_deref()
        && let Some(declared) = declared_constraint(doc, id)
    {
        return if same_stanza(declared, src) {
            Ok(Location::AlreadyPresent)
        } else {
            Err(format!(
                "constraint {id:?} is already declared in {OVERLAY_FILE} with another body: a candidate adds rules and never edits one (edit it by hand, or give this one another id)"
            ))
        };
    }
    place_table(doc, cand, r, next)
}

/// The first `[[constraint]]` table (or inline table of a `constraint =
/// [...]` array) of `doc` whose `id` is `id`: the one the loader keeps.
fn declared_constraint<'d>(doc: &'d DocumentMut, id: &str) -> Option<&'d dyn TableLike> {
    match doc.get("constraint")? {
        Item::ArrayOfTables(tables) => tables
            .iter()
            .find(|t| t.get("id").and_then(Item::as_str) == Some(id))
            .map(|t| t as &dyn TableLike),
        Item::Value(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_inline_table)
            .find(|t| t.get("id").and_then(Value::as_str) == Some(id))
            .map(|t| t as &dyn TableLike),
        _ => None,
    }
}

/// Two stanza bodies are the same: the same keys, each with an equal value
/// ([`canon_item`]); formatting, key order and comments ignored.
fn same_stanza(a: &dyn TableLike, b: &dyn TableLike) -> bool {
    canon_table(a) == canon_table(b)
}

/// A table's value, independent of how it is written: its `key=value` pairs
/// sorted by key.
fn canon_table(t: &dyn TableLike) -> String {
    let mut pairs: Vec<String> = t
        .iter()
        .map(|(k, v)| format!("{k:?}={}", canon_item(v)))
        .collect();
    pairs.sort();
    format!("{{{}}}", pairs.join(","))
}

fn canon_item(item: &Item) -> String {
    match item {
        Item::Value(v) => canon(v),
        Item::Table(t) => canon_table(t),
        Item::ArrayOfTables(tables) => {
            let items: Vec<String> = tables.iter().map(|t| canon_table(t)).collect();
            format!("[{}]", items.join(","))
        }
        Item::None => String::new(),
    }
}

/// A value without its formatting: a string as its Rust debug form, a number,
/// bool or datetime as itself, an array item by item, an inline table as
/// [`canon_table`].
fn canon(v: &Value) -> String {
    match v {
        Value::String(s) => format!("{:?}", s.value()),
        Value::Integer(n) => n.value().to_string(),
        Value::Float(f) => f.value().to_string(),
        Value::Boolean(b) => b.value().to_string(),
        Value::Datetime(d) => d.value().to_string(),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(canon).collect();
            format!("[{}]", items.join(","))
        }
        Value::InlineTable(t) => canon_table(t),
    }
}

/// Raise `doc`'s top-level integer `version` to `to` in place, its decor
/// (the ` # comment` after it) kept; the old version when it was raised.
/// A missing, non-integer or already-high `version` is left as it is: a
/// version is never added (that would start loading a file the loader
/// ignores) and never lowered.
fn require_version(doc: &mut DocumentMut, to: i64) -> Option<i64> {
    let v = doc.get_mut("version").and_then(Item::as_value_mut)?;
    let old = v.as_integer().filter(|n| *n < to)?;
    let decor = v.decor().clone();
    let mut raised = Value::from(to);
    *raised.decor_mut() = decor;
    *v = raised;
    Some(old)
}

/// `Err` when raising the base's `version` (from `old`) would by itself load
/// more stanzas than the base does: a stanza `old` drops, typically a
/// hand-written `rule` / `transitive` / `via` / `min_tier` in a version-1
/// file, would start applying unseen. The merged file must differ from the
/// base only by the stanzas written and the version line.
fn hidden_stanzas(base: &LoadedConfig, raised: &LoadedConfig, old: i64) -> Result<(), String> {
    let more: Vec<String> = base
        .section_counts()
        .into_iter()
        .zip(raised.section_counts())
        .filter(|((_, b), (_, r))| r != b)
        .map(|((section, b), (_, r))| format!("{section}={}", r.abs_diff(b)))
        .collect();
    if more.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the merged {OVERLAY_FILE} would also load {} stanza(s) that version = {old} drops ({}): raise the version by hand, check those stanzas, then accept again",
        more.join(" "),
        unlocated(base).join("; ")
    ))
}

/// Insert the candidate's constant into `doc`'s `[constants]` (created at the
/// end when absent), its key's comment block kept.
fn place_constant(
    doc: &mut DocumentMut,
    cand: &DocumentMut,
    r: &StanzaRef,
    next: &mut usize,
) -> Result<Location, String> {
    let (key, item) = r
        .key
        .as_deref()
        .and_then(|name| {
            cand.get("constants")
                .and_then(Item::as_table)
                .and_then(|t| t.get_key_value(name))
        })
        .ok_or_else(|| format!("stanza {r} is not in the candidate"))?;
    let name = key.get().to_string();
    let constants = section_table(doc, "constants", next)?;
    if let Some(pinned) = constants.get(&name) {
        return match (pinned.as_str(), item.as_str()) {
            (Some(p), Some(v)) if p == v => Ok(Location::AlreadyPresent),
            (Some(p), _) => Err(format!("constant {name} already pinned to {p:?}")),
            (None, _) => Err(format!(
                "constant {name} already pinned to {}",
                pinned.to_string().trim()
            )),
        };
    }
    let mut key = key.clone();
    let block = comment_block(key.leaf_decor().prefix().and_then(|p| p.as_str())).to_string();
    key.leaf_decor_mut().set_prefix(block);
    constants.insert_formatted(&key, item.clone());
    Ok(Location::Constant { name })
}

/// Append the candidate's pattern to `doc`'s `[entrypoints] qnames` (created
/// at the end when absent). In a multi-line array (or a new one) the item
/// gets its own line with its `# gap:` lines above it; appended to a
/// one-line array it goes inline, without them.
fn place_pattern(
    doc: &mut DocumentMut,
    r: &StanzaRef,
    next: &mut usize,
) -> Result<Location, String> {
    let pattern = r
        .key
        .clone()
        .ok_or_else(|| format!("stanza {r} has no pattern"))?;
    let entrypoints = section_table(doc, "entrypoints", next)?;
    if !entrypoints.contains_key("qnames") {
        let mut qnames = Array::new();
        qnames.set_trailing_comma(true);
        qnames.set_trailing("\n");
        entrypoints.insert("qnames", Item::Value(Value::Array(qnames)));
    }
    let qnames = entrypoints
        .get_mut("qnames")
        .and_then(Item::as_array_mut)
        .ok_or_else(|| {
            format!("{OVERLAY_FILE} [entrypoints] qnames is not an array; merge by hand")
        })?;
    if qnames.iter().any(|v| v.as_str() == Some(pattern.as_str())) {
        return Ok(Location::AlreadyPresent);
    }
    let last_prefix = qnames.iter().last().map(|v| {
        v.decor()
            .prefix()
            .and_then(|p| p.as_str())
            .unwrap_or_default()
    });
    let mut item = Value::from(pattern.as_str());
    match last_prefix {
        Some(p) if !p.contains('\n') => item.decor_mut().set_prefix(" "),
        _ => {
            // Own line: the indent of the last item's line (or the default).
            let indent = last_prefix
                .and_then(|p| p.rsplit('\n').next())
                .filter(|i| i.chars().all(|c| c == ' ' || c == '\t'))
                .unwrap_or(ITEM_INDENT);
            let mut prefix = String::from("\n");
            for g in &r.gaps {
                prefix.push_str(&format!("{indent}# gap: {g}\n"));
            }
            prefix.push_str(indent);
            item.decor_mut().set_prefix(prefix);
            if qnames.is_empty() {
                qnames.set_trailing_comma(true);
                qnames.set_trailing("\n");
            }
        }
    }
    item.decor_mut().set_suffix("");
    qnames.push_formatted(item);
    Ok(Location::Entrypoint {
        index: qnames.len() - 1,
        pattern,
    })
}

/// `doc`'s `[name]` table, created at the end of the document when absent.
fn section_table<'d>(
    doc: &'d mut DocumentMut,
    name: &str,
    next: &mut usize,
) -> Result<&'d mut Table, String> {
    if !doc.contains_key(name) {
        let mut table = Table::new();
        table.set_position(bump(next));
        doc.insert(name, Item::Table(table));
    }
    doc.get_mut(name)
        .and_then(Item::as_table_mut)
        .ok_or_else(|| format!("{OVERLAY_FILE} writes `{name}` as something other than a [{name}] table; merge by hand"))
}

/// The next free document position (a new table at the end).
fn bump(next: &mut usize) -> usize {
    let p = *next;
    *next += 1;
    p
}
