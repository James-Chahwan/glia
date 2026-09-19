//! ADR ingest (LF.4b): a DECISION entry on every node an architecture
//! decision record DOCUMENTS.
//!
//! The doc ingest ([`crate::docs`]) admits ADR directories
//! ([`is_adr_path`]: `doc/adr/`, `adr/`, `docs/decisions/`,
//! `architecture/decisions/`, ...) and splits each ADR into DOC_SECTIONs; the
//! doc linker (`passes::link_doc_sections`) draws DOCUMENTS edges from a
//! section to the symbols its backticks name. [`fill_adr_decisions`] runs
//! right after the linker, as the `fill_adr_decisions` Post pass of
//! [`crate::profile::CODE_PASSES`], and lifts "what was decided about X" onto
//! X itself: one DECISION entry per (node, ADR), however many of the ADR's
//! sections mention the node,
//! `{"source":"adr","id":"adr:<file>","adr":"<file>","title":..,"status":..,"section":<qname>}`,
//! upserted through `external_inputs::merge_entry` - the canonical entry
//! array the `.glia/cells.jsonl` sidecar (LF.1a) and the overlay's declared
//! decisions (LF.4a) merge into later in the build.
//!
//! An ADR file is a repo markdown file (File provenance: external pages are
//! not ADRs) that is either
//! - under an ADR directory, unless it is the directory's index or template
//!   (`README` / `index` / `toc` / `*template*`), or
//! - named `NNN-...` / `NNNN-...` and shaped like one: a `status` section and
//!   a `decision` / `decision-outcome` section.
//!
//! Its title is its first heading, leading `#`s and an adr-tools ordinal
//! (`1. `) stripped (the file stem when it has none). Its status is the first
//! word of the `status` section's first line, else of a `Status: X` /
//! `* Status: X` line (MADR 2) or a `status: X` front-matter key (MADR 3) in
//! the head of the file, lowercased; absent when none says. Its decision
//! section is `decision`, else `decision-outcome`, else the first
//! `decision*` section, else the first section.
//!
//! Deterministic: ADR files are visited in (repo, path) order and nodes in
//! NodeId order; every instance of a node (one id can sit in several language
//! graphs) gets the same entry. Idempotent: an entry upserts by
//! `(source, id)`, so a re-run over a loaded graph changes nothing.
//!
//! fired_on marker, once per build that has an ADR:
//!   `[adr] files=<f> decisions=<d> nodes=<n> (accepted=<a> proposed=<p> superseded=<s> deprecated=<x> rejected=<r> other=<o>)`
//! `decisions` counts the (node, ADR) entries written, `nodes` the nodes that
//! got one, and the statuses count ADR FILES (they sum to `files`; `other`
//! holds any other word and a missing status). A trailing ` skipped=<k>`
//! appears only when (node, ADR) pairs were not written: an entry that fails
//! `validate_entry` (an `id` over 128 chars) or a DECISION cell that is not
//! an entry array.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use repo_graph_code_domain::external_inputs::{merge_entry, validate_entry};
use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, NodeId};
use repo_graph_graph::MergedGraph;
use serde_json::{Map, Value};

use crate::docs::is_adr_path;

/// The status words the marker counts by name; any other is `other`.
const STATUSES: [&str; 5] = ["accepted", "proposed", "superseded", "deprecated", "rejected"];

/// One DOC_SECTION of a repo markdown file.
struct Section {
    id: NodeId,
    slug: String,
    qname: String,
    text: String,
    start_line: u64,
}

/// One detected ADR file.
struct Adr {
    file: String,
    title: String,
    status: Option<String>,
    /// Qname of its decision section.
    section: String,
}

impl Adr {
    /// The DECISION entry this ADR puts on every node it documents.
    fn entry(&self) -> Value {
        let mut m = Map::new();
        m.insert("source".into(), Value::from("adr"));
        m.insert("id".into(), Value::from(format!("adr:{}", self.file)));
        m.insert("adr".into(), Value::from(self.file.clone()));
        m.insert("title".into(), Value::from(self.title.clone()));
        if let Some(status) = &self.status {
            m.insert("status".into(), Value::from(status.clone()));
        }
        m.insert("section".into(), Value::from(self.section.clone()));
        Value::Object(m)
    }
}

/// What [`fill_adr_decisions`] did, for the `[adr]` marker.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct AdrStats {
    /// ADR files detected.
    files: usize,
    /// (node, ADR) entries written.
    decisions: usize,
    /// Nodes that got at least one entry.
    nodes: usize,
    /// ADR files per status: [`STATUSES`] in order, then `other`.
    statuses: [usize; 6],
    /// (node, ADR) pairs not written (invalid entry, or a DECISION cell that
    /// is not an entry array).
    skipped: usize,
}

impl AdrStats {
    /// The LF.4b fired_on marker (module docs); silent without an ADR.
    pub(crate) fn report(&self) {
        if self.files == 0 {
            return;
        }
        let [accepted, proposed, superseded, deprecated, rejected, other] = self.statuses;
        let skipped = if self.skipped > 0 { format!(" skipped={}", self.skipped) } else { String::new() };
        eprintln!(
            "[adr] files={} decisions={} nodes={} (accepted={accepted} proposed={proposed} \
             superseded={superseded} deprecated={deprecated} rejected={rejected} other={other}){skipped}",
            self.files, self.decisions, self.nodes
        );
    }
}

/// Put one DECISION entry per (node, ADR) on every node an ADR section
/// DOCUMENTS (module docs). Runs after `link_doc_sections`, whose edges it
/// reads.
pub(crate) fn fill_adr_decisions(merged: &mut MergedGraph) -> AdrStats {
    let mut stats = AdrStats::default();
    let (adrs, section_adr) = find_adrs(merged, &mut stats);
    if adrs.is_empty() {
        return stats;
    }

    // Documented node (raw id, for NodeId order) -> the ADRs documenting it,
    // by index into `adrs` (path order).
    let mut targets: BTreeMap<u64, BTreeSet<usize>> = BTreeMap::new();
    for e in merged.all_edges() {
        if e.category != edge_category::DOCUMENTS || e.from == e.to {
            continue;
        }
        if let Some(&adr) = section_adr.get(&e.from) {
            targets.entry(e.to.0).or_default().insert(adr);
        }
    }
    if targets.is_empty() {
        return stats;
    }

    // Every instance of every target: (graph index, node index).
    let mut at: HashMap<NodeId, Vec<(usize, usize)>> = HashMap::new();
    for (gi, g) in merged.graphs.iter().enumerate() {
        for (ni, n) in g.nodes.iter().enumerate() {
            if targets.contains_key(&n.id.0) {
                at.entry(n.id).or_default().push((gi, ni));
            }
        }
    }

    let entries: Vec<Option<Value>> = adrs
        .iter()
        .map(|a| {
            let v = a.entry();
            validate_entry(cell_type::DECISION, &v).is_ok().then_some(v)
        })
        .collect();

    for (raw, adr_ids) in &targets {
        let Some(places) = at.get(&NodeId(*raw)) else {
            continue;
        };
        let mut got_one = false;
        for &adr in adr_ids {
            let Some(entry) = entries.get(adr).and_then(Option::as_ref) else {
                stats.skipped += 1;
                continue;
            };
            let mut wrote = false;
            for &(gi, ni) in places {
                let Some(node) = merged.graphs.get_mut(gi).and_then(|g| g.nodes.get_mut(ni)) else {
                    continue;
                };
                let slot = node.cells.iter().position(|c| c.kind == cell_type::DECISION);
                let existing = slot.and_then(|i| node.cells.get(i)).map(|c| &c.payload);
                let Ok(payload) = merge_entry(existing, entry) else {
                    continue;
                };
                match slot.and_then(|i| node.cells.get_mut(i)) {
                    Some(cell) => cell.payload = payload,
                    None => node.cells.push(Cell { kind: cell_type::DECISION, payload }),
                }
                wrote = true;
            }
            if wrote {
                stats.decisions += 1;
                got_one = true;
            } else {
                stats.skipped += 1;
            }
        }
        stats.nodes += usize::from(got_one);
    }
    stats
}

/// The ADR files of `merged` in (repo, path) order, and each of their
/// sections' index into that list. Counts files and statuses into `stats`.
fn find_adrs(merged: &MergedGraph, stats: &mut AdrStats) -> (Vec<Adr>, HashMap<NodeId, usize>) {
    let mut files: BTreeMap<(u64, String), Vec<Section>> = BTreeMap::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id).copied() != Some(node_kind::DOC_SECTION)
                || !is_file_doc(&n.cells)
                || !seen.insert(n.id)
            {
                continue;
            }
            let (Some((file, start_line)), Some(text)) = (position(&n.cells), code_text(&n.cells)) else {
                continue;
            };
            let slug = g.nav.name_by_id.get(&n.id).cloned().unwrap_or_default();
            let qname = g.nav.qname_by_id.get(&n.id).cloned().unwrap_or_default();
            files
                .entry((n.repo.0, file.replace('\\', "/")))
                .or_default()
                .push(Section { id: n.id, slug, qname, text: text.to_string(), start_line });
        }
    }

    let mut adrs = Vec::new();
    let mut section_adr = HashMap::new();
    for ((_, file), mut sections) in files {
        sections.sort_by(|a, b| (a.start_line, &a.qname).cmp(&(b.start_line, &b.qname)));
        if !is_adr_file(&file, &sections) {
            continue;
        }
        let Some(section) = decision_section(&sections).map(|s| s.qname.clone()) else {
            continue;
        };
        let status = adr_status(&sections);
        let bucket = status
            .as_deref()
            .and_then(|s| STATUSES.iter().position(|k| *k == s))
            .unwrap_or(STATUSES.len());
        if let Some(n) = stats.statuses.get_mut(bucket) {
            *n += 1;
        }
        stats.files += 1;
        let idx = adrs.len();
        for s in &sections {
            section_adr.insert(s.id, idx);
        }
        adrs.push(Adr { title: adr_title(&sections, &file), status, section, file });
    }
    (adrs, section_adr)
}

/// A repo markdown file's section: ORIGIN `provenance: documentation` with
/// no external `source` (an ingested page carries `confluence` / `notion` /
/// `wiki`; a contract op says `provenance: contract`).
fn is_file_doc(cells: &[Cell]) -> bool {
    cells.iter().any(|c| match &c.payload {
        CellPayload::Json(s) if c.kind == cell_type::ORIGIN => serde_json::from_str::<Value>(s)
            .is_ok_and(|v| {
                v.get("provenance").and_then(Value::as_str) == Some("documentation")
                    && v.get("source").and_then(Value::as_str).is_none_or(|t| t == "file")
            }),
        _ => false,
    })
}

/// `(file, start_line)` of a DOC_SECTION's POSITION cell.
fn position(cells: &[Cell]) -> Option<(String, u64)> {
    cells.iter().find_map(|c| match &c.payload {
        CellPayload::Json(s) if c.kind == cell_type::POSITION => {
            let v: Value = serde_json::from_str(s).ok()?;
            Some((v.get("file")?.as_str()?.to_string(), v.get("start_line")?.as_u64()?))
        }
        _ => None,
    })
}

/// A DOC_SECTION's prose (its CODE cell).
fn code_text(cells: &[Cell]) -> Option<&str> {
    cells.iter().find_map(|c| match &c.payload {
        CellPayload::Text(s) if c.kind == cell_type::CODE => Some(s.as_str()),
        _ => None,
    })
}

/// Is this file an ADR (module docs)? `sections` is its sections in order.
fn is_adr_file(file: &str, sections: &[Section]) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s).to_ascii_lowercase();
    let index = matches!(stem.as_str(), "readme" | "index" | "toc") || stem.contains("template");
    if is_adr_path(file) && !index {
        return true;
    }
    let has = |slug: &str| sections.iter().any(|s| s.slug == slug);
    numbered(name) && has("status") && (has("decision") || has("decision-outcome"))
}

/// `NNN-...` / `NNNN-...`: an ADR sequence number.
fn numbered(name: &str) -> bool {
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    (3..=4).contains(&digits) && name.as_bytes().get(digits) == Some(&b'-')
}

/// `decision`, else `decision-outcome` (MADR), else the first `decision*`
/// section, else the first section.
fn decision_section(sections: &[Section]) -> Option<&Section> {
    sections
        .iter()
        .find(|s| s.slug == "decision")
        .or_else(|| sections.iter().find(|s| s.slug == "decision-outcome"))
        .or_else(|| sections.iter().find(|s| s.slug.starts_with("decision")))
        .or_else(|| sections.first())
}

/// The first non-blank line of a section.
fn first_line(s: &Section) -> Option<&str> {
    s.text.lines().find(|l| !l.trim().is_empty())
}

/// The first heading, `#`s and a `N. ` ordinal stripped; the file stem when
/// no section starts with one.
fn adr_title(sections: &[Section], file: &str) -> String {
    for s in sections {
        let Some(line) = first_line(s).map(str::trim_start) else {
            continue;
        };
        if !line.starts_with('#') {
            continue;
        }
        let title = strip_ordinal(line.trim_start_matches('#').trim());
        if !title.is_empty() {
            return title.to_string();
        }
    }
    let name = file.rsplit('/').next().unwrap_or(file);
    name.rsplit_once('.').map_or(name, |(s, _)| s).to_string()
}

/// `1. Use Flask` -> `Use Flask`; anything else as is.
fn strip_ordinal(t: &str) -> &str {
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    match t.get(digits..).and_then(|r| r.strip_prefix('.')) {
        Some(rest) if digits > 0 && rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => t,
    }
}

/// The ADR's status (module docs), lowercased, one word.
fn adr_status(sections: &[Section]) -> Option<String> {
    if let Some(s) = sections.iter().find(|s| s.slug == "status") {
        // Skip the `## Status` heading itself.
        let line = s.text.lines().skip(1).find(|l| !l.trim().is_empty());
        if let Some(word) = line.and_then(|l| status_word(status_label(l).unwrap_or(l))) {
            return Some(word);
        }
    }
    // The head: any preamble (front matter) and the first heading section.
    let head = sections
        .iter()
        .position(|s| first_line(s).is_some_and(|l| l.trim_start().starts_with('#')))
        .map_or(sections.len(), |i| i + 1);
    sections
        .iter()
        .take(head)
        .flat_map(|s| s.text.lines())
        .find_map(|l| status_label(l).and_then(status_word))
}

/// What follows a `Status:` label: `Status: X`, `* Status: X`,
/// `* **Status:** X`, `**Status**: X`, front-matter `status: X`.
fn status_label(line: &str) -> Option<&str> {
    let l = line.trim().trim_start_matches(['*', '-', '+', '_', ' ', '\t']);
    let head = l.get(..6)?;
    if !head.eq_ignore_ascii_case("status") {
        return None;
    }
    l.get(6..)?.trim_start_matches(['*', '_', ' ']).strip_prefix(':')
}

/// The first alphabetic word, lowercased: `**Accepted**` -> `accepted`,
/// `Superseded by [3. ...]` -> `superseded`.
fn status_word(s: &str) -> Option<String> {
    let word: String = s
        .chars()
        .skip_while(|c| !c.is_alphabetic())
        .take_while(|c| c.is_alphabetic())
        .collect();
    (!word.is_empty()).then(|| word.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sections(text: &[(&str, &str)]) -> Vec<Section> {
        text.iter()
            .enumerate()
            .map(|(i, (slug, text))| Section {
                id: NodeId(i as u64),
                slug: (*slug).to_string(),
                qname: format!("docs::f::{slug}"),
                text: (*text).to_string(),
                start_line: i as u64 * 10,
            })
            .collect()
    }

    /// adr-tools' template (`adr new`), as the fixture writes it.
    fn adr_tools() -> Vec<Section> {
        sections(&[
            ("1-use-flask-for-orders", "# 1. Use Flask for orders"),
            ("status", "## Status\n\nAccepted"),
            ("context", "## Context\n\nWe need a small HTTP layer."),
            ("decision", "## Decision\n\nServe `list_orders` from Flask."),
            ("consequences", "## Consequences\n\nSimple."),
        ])
    }

    #[test]
    fn adr_tools_title_status_and_decision_section() {
        let s = adr_tools();
        assert_eq!(adr_title(&s, "doc/adr/0001-use-flask-for-orders.md"), "Use Flask for orders");
        assert_eq!(adr_status(&s).as_deref(), Some("accepted"));
        assert_eq!(decision_section(&s).map(|s| s.qname.as_str()), Some("docs::f::decision"));
    }

    #[test]
    fn status_forms() {
        // A superseded adr-tools record names its successor on the same line.
        let s = sections(&[
            ("t", "# 2. X"),
            ("status", "## Status\n\nSuperseded by [3. Y](0003-y.md)"),
        ]);
        assert_eq!(adr_status(&s).as_deref(), Some("superseded"));
        // MADR 2: a `* Status:` bullet in the title section.
        let s = sections(&[("t", "# Use Postgres\n\n* Status: proposed\n* Deciders: team")]);
        assert_eq!(adr_status(&s).as_deref(), Some("proposed"));
        // Bold labels, and a label inside the status section.
        let s = sections(&[("t", "# T\n\n* **Status:** Deprecated")]);
        assert_eq!(adr_status(&s).as_deref(), Some("deprecated"));
        let s = sections(&[("t", "# T"), ("status", "## Status\nStatus: **Rejected**")]);
        assert_eq!(adr_status(&s).as_deref(), Some("rejected"));
        // MADR 3: YAML front matter is the preamble before the title.
        let s = sections(&[
            ("overview", "---\nstatus: \"accepted\"\ndate: 2026-01-01\n---"),
            ("use-postgres", "# Use Postgres"),
        ]);
        assert_eq!(adr_status(&s).as_deref(), Some("accepted"));
        assert_eq!(adr_title(&s, "docs/decisions/0002-use-postgres.md"), "Use Postgres");
        // No status anywhere; `Status quo` is not a label; a later section's
        // label is not the head's.
        let s = sections(&[("t", "# T\n\nStatus quo: fine."), ("more", "## More\nStatus: accepted")]);
        assert_eq!(adr_status(&s), None);
    }

    #[test]
    fn title_falls_back_to_the_stem() {
        let s = sections(&[("overview", "no heading here")]);
        assert_eq!(adr_title(&s, "adr/0009-plain.md"), "0009-plain");
        assert_eq!(strip_ordinal("12. Twelve"), "Twelve");
        assert_eq!(strip_ordinal("12.5 percent"), "12.5 percent");
        assert_eq!(strip_ordinal("2024 plan"), "2024 plan");
    }

    #[test]
    fn madr_decision_outcome_beats_decision_drivers() {
        let s = sections(&[
            ("t", "# T"),
            ("decision-drivers", "## Decision Drivers"),
            ("decision-outcome", "## Decision Outcome"),
        ]);
        assert_eq!(decision_section(&s).map(|s| s.slug.as_str()), Some("decision-outcome"));
        let s = sections(&[("t", "# T"), ("context", "## Context")]);
        assert_eq!(decision_section(&s).map(|s| s.slug.as_str()), Some("t"));
    }

    #[test]
    fn adr_file_detection() {
        let shaped = adr_tools();
        let plain = sections(&[("t", "# Notes"), ("setup", "## Setup")]);
        // Any file of an ADR directory but its index / template.
        assert!(is_adr_file("doc/adr/0001-use-flask-for-orders.md", &shaped));
        assert!(is_adr_file("docs/decisions/use-postgres.md", &plain));
        assert!(!is_adr_file("doc/adr/README.md", &plain));
        assert!(!is_adr_file("docs/decisions/adr-template.md", &plain));
        assert!(!is_adr_file("docs/adr/index.md", &plain));
        // Elsewhere: a sequence number AND the ADR shape.
        assert!(is_adr_file("docs/architecture/0005-queue-choice.md", &shaped));
        assert!(!is_adr_file("docs/architecture/0005-queue-choice.md", &plain));
        assert!(!is_adr_file("docs/architecture/queue-choice.md", &shaped));
        assert!(!is_adr_file("docs/12-steps.md", &shaped));
        assert!(!is_adr_file("docs/20260101-log.md", &shaped));
        // An index that is numbered and shaped is still an ADR.
        assert!(is_adr_file("doc/adr/0000-template.md", &shaped));
    }

    #[test]
    fn entry_shape_is_canonical() {
        let adr = Adr {
            file: "doc/adr/0001-x.md".into(),
            title: "X".into(),
            status: None,
            section: "docs::doc::adr::0001-x::decision".into(),
        };
        let v = adr.entry();
        assert!(validate_entry(cell_type::DECISION, &v).is_ok());
        assert_eq!(v.get("status"), None, "a missing status is omitted, not invented");
        let Ok(CellPayload::Json(p)) = merge_entry(None, &v) else {
            panic!("an entry merges into an empty cell");
        };
        assert_eq!(
            p,
            r#"[{"adr":"doc/adr/0001-x.md","id":"adr:doc/adr/0001-x.md","section":"docs::doc::adr::0001-x::decision","source":"adr","title":"X"}]"#
        );
    }
}
