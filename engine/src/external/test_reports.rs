//! The test-report stage (LF.6b): a repo's `.glia/test-snapshot/` (LF.6a,
//! written by `glia tests ingest`, read back through
//! `code_domain::snapshots::read_tests`) becomes FAIL cells on the failing
//! tests and on the code their traces implicate. A test report is a FACT
//! input: the stage runs with or without `--no-overlay`. A repo with no
//! snapshot directory is a no-op that prints nothing; an incomplete snapshot
//! is `read_tests`' `[tests] snapshot incomplete` line and a no-op.
//!
//! Every case, message and trace comes back from `read_tests` already passed
//! through `snapshots::redact_untrusted` (A13.7's denylist plus the secret
//! shapes), whatever the file holds. The few free-text fields `read_tests`
//! does not sanitise (`source`, `report`, the meta's `run`) go through the
//! same helper here, so no stored string skips it. The trace itself is never
//! stored: it is only resolved.
//!
//! MAPPING. Every item is resolved in ONE [`MergedGraph::resolve_signals`]
//! batch per repo (one `[resolve] batch` line). Per case, first hit wins:
//! 1. `file_line` (FACT): the case's `file` and 1-based `line` as a
//!    stacktrace frame; the hit counts when it is a FUNCTION / METHOD whose
//!    POSITION file is the case's file (a boundary-aligned path suffix
//!    either way), never a basename look-alike.
//! 2. `qname` (FACT): segments = `classname` split on `. : / \` (a trailing
//!    source-extension segment dropped) + `name` split on `::` (its last
//!    segment cut before a parameter suffix `[`, `(` or a subtest `/`); each
//!    suffix `segs[k..]` of at least two segments, longest first, is a `test`
//!    item, accepted when the hit's qname IS the token or ends with
//!    `::<token>` (resolve's bare-name fallback is not a qname match). Several
//!    nodes carry that qname: the ones whose POSITION file is the case's
//!    `file` first, then `MergedGraph::pick_primary`.
//! 3. `name` (HEURISTIC): the bare (cut) name, only when exactly one FUNCTION
//!    / METHOD of the repo carries it.
//!
//! Only the repo's own nodes map. A case the ladder misses is counted
//! `unmapped`. A failure reported more than once is kept once, the JUnit copy
//! first: cases with one classname + name (a JUnit case and its CI-log copy,
//! or one test in two reports of the run), and a JUnit and a log case of one
//! name that map to the same test (a log line that gave no classname).
//!
//! IMPLICATED (FACT given the trace): the case's trace resolved as a
//! stacktrace; its nodes in resolution order (index = `frame`), keeping those
//! of this repo that are not the test node, not in the test node's file and
//! carry no ORIGIN provenance `test_fixture` (`passes::tag_synthetic_provenance`,
//! which also tags `tests::`-rooted qnames), at most [`MAX_IMPLICATED`] per
//! case. A case whose test did not map still implicates its frames.
//!
//! PAYLOAD. FAIL (cell 9) is the canonical entry array of
//! `external_inputs::merge_entry` (sorted by `(source, id)`, compact, sorted
//! keys). It is not in `external_inputs::WRITABLE`: this stage is its only
//! writer. The failing test's entry:
//! `{"id":"<run|latest>:<classname>::<name>","message"?,"redacted"?,"report","role":"test","run"?,"source":"junit|log","status":"failed|error","test":"<classname>::<name>","via":"file_line|qname|name"}`;
//! each implicated node's: the same id plus `#<frame>`, `"role":"implicated"`,
//! `"frame":<frame>` and no `via`. `redacted: true` is set when the case's
//! text was redacted (A13.7's cell convention). A node keeps at most
//! [`MAX_ENTRIES_PER_NODE`] entries, the lowest `(source, id)` first; the
//! rest are counted `dropped`.
//!
//! Marker, once per repo with a complete snapshot (the fired_on line):
//!   `[tests] fail-cells repo=<label> cases=<n> mapped=<m> (file_line=<a> qname=<b> name=<c>) unmapped=<u> implicated=<i> fail_cells=<f> dropped=<d>`
//! where `cases` counts distinct failures (`mapped + unmapped`), `implicated`
//! the implicated entries and `fail_cells` the nodes whose FAIL cell this
//! stage wrote. When a repeated case was folded, one more line follows:
//!   `[tests] fail-cells deduplicated <n> repeated case(s) repo=<label>`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use repo_graph_code_domain::external_inputs::merge_entry;
use repo_graph_code_domain::snapshots::{SOURCE_JUNIT, TestCaseRecord, read_tests, redact_untrusted};
use repo_graph_code_domain::{cell_type, node_kind};
use repo_graph_core::{Cell, CellPayload, Node, NodeId, NodeKindId};
use repo_graph_graph::MergedGraph;
use serde_json::{Map, Value};

use super::RepoInputs;

/// At most this many FAIL entries on one node; the lowest `(source, id)` stay.
pub(crate) const MAX_ENTRIES_PER_NODE: usize = 20;

/// At most this many implicated nodes per case, in resolution order.
pub(crate) const MAX_IMPLICATED: usize = 5;

/// The kinds a failing test maps to on the `file_line` and `name` rungs.
const TEST_KINDS: [NodeKindId; 2] = [node_kind::FUNCTION, node_kind::METHOD];

/// A classname's trailing segment that names a source file's extension
/// (`tests/test_app.py` -> `tests`, `test_app`).
const SOURCE_EXTENSIONS: &[&str] = &[
    "py", "go", "rs", "java", "kt", "kts", "scala", "groovy", "ts", "tsx", "js", "jsx", "mjs", "cjs", "rb",
    "php", "cs", "swift", "dart", "ex", "exs", "sol", "c", "cc", "cpp", "cxx", "h", "hpp", "clj", "cljs",
    "vue",
];

/// How a failing test was mapped to its node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    FileLine,
    Qname,
    Name,
}

impl Via {
    fn as_str(self) -> &'static str {
        match self {
            Via::FileLine => "file_line",
            Via::Qname => "qname",
            Via::Name => "name",
        }
    }
}

/// What the stage needs to know about one node of the repo.
struct Facts<'a> {
    kind: Option<NodeKindId>,
    qname: &'a str,
    /// The file of its first POSITION cell.
    file: Option<String>,
    /// ORIGIN provenance `test_fixture`.
    fixture: bool,
}

/// The repo's nodes, indexed for the ladder. Built once per snapshot.
struct RepoIndex<'a> {
    facts: HashMap<NodeId, Facts<'a>>,
    /// qname's last `::` segment -> nodes, sorted by id.
    by_tail: HashMap<&'a str, Vec<NodeId>>,
    /// simple name -> FUNCTION / METHOD nodes, sorted by id.
    tests_by_name: HashMap<&'a str, Vec<NodeId>>,
}

impl<'a> RepoIndex<'a> {
    fn new(merged: &'a MergedGraph, input: &RepoInputs) -> Self {
        let mut facts = HashMap::new();
        let mut by_tail: HashMap<&str, Vec<NodeId>> = HashMap::new();
        let mut tests_by_name: HashMap<&str, Vec<NodeId>> = HashMap::new();
        for g in merged.graphs.iter().filter(|g| g.repo == input.repo) {
            for n in &g.nodes {
                if facts.contains_key(&n.id) {
                    continue;
                }
                let kind = g.nav.kind_by_id.get(&n.id).copied();
                let qname = g.nav.qname_by_id.get(&n.id).map_or("", String::as_str);
                if !qname.is_empty() {
                    by_tail.entry(qname.rsplit("::").next().unwrap_or(qname)).or_default().push(n.id);
                }
                if kind.is_some_and(|k| TEST_KINDS.contains(&k))
                    && let Some(name) = g.nav.name_by_id.get(&n.id)
                {
                    tests_by_name.entry(name.as_str()).or_default().push(n.id);
                }
                facts.insert(n.id, Facts { kind, qname, file: position_file(n), fixture: is_fixture(n) });
            }
        }
        for ids in by_tail.values_mut().chain(tests_by_name.values_mut()) {
            ids.sort_by_key(|id| id.0);
        }
        RepoIndex { facts, by_tail, tests_by_name }
    }

    fn file_is(&self, id: NodeId, file: &str) -> bool {
        self.facts.get(&id).and_then(|f| f.file.as_deref()).is_some_and(|f| same_file(f, file))
    }
}

/// One case's items in the resolve batch.
#[derive(Default)]
struct CaseItems {
    file_line: Option<usize>,
    /// `(token, item)`, longest token first.
    tokens: Vec<(String, usize)>,
    trace: Option<usize>,
}

/// The stage's counts, for its marker.
#[derive(Debug, Default, PartialEq, Eq)]
struct Tally {
    cases: usize,
    file_line: usize,
    qname: usize,
    name: usize,
    unmapped: usize,
    implicated: usize,
    fail_cells: usize,
    dropped: usize,
    deduplicated: usize,
}

impl Tally {
    fn mapped(&self) -> usize {
        self.file_line + self.qname + self.name
    }
}

/// Apply `input`'s test-report snapshot: FAIL cells on the repo's failing
/// tests and on the nodes their traces implicate, then the stage marker.
/// True when a cell was written.
pub(super) fn ingest_test_reports(merged: &mut MergedGraph, input: &RepoInputs) -> bool {
    let Some(snapshot) = read_tests(&input.root) else {
        return false;
    };
    let run = snapshot.meta.run.as_deref().map(redact_untrusted);
    let run = run.as_ref().map(|(r, spans)| (r.as_str(), *spans));
    let (plan, mut tally) = plan_fail_cells(merged, input, &snapshot.cases, run);
    write_fail_cells(merged, input, plan, &mut tally);
    eprintln!(
        "[tests] fail-cells repo={} cases={} mapped={} (file_line={} qname={} name={}) unmapped={} implicated={} fail_cells={} dropped={}",
        input.label,
        tally.cases,
        tally.mapped(),
        tally.file_line,
        tally.qname,
        tally.name,
        tally.unmapped,
        tally.implicated,
        tally.fail_cells,
        tally.dropped
    );
    if tally.deduplicated > 0 {
        eprintln!(
            "[tests] fail-cells deduplicated {} repeated case(s) repo={}",
            tally.deduplicated, input.label
        );
    }
    tally.fail_cells > 0
}

/// Node id -> the FAIL entries it takes, plus the mapping counts.
fn plan_fail_cells(
    merged: &MergedGraph,
    input: &RepoInputs,
    cases: &[TestCaseRecord],
    run: Option<(&str, usize)>,
) -> (BTreeMap<u64, Vec<Value>>, Tally) {
    let mut tally = Tally::default();
    let cases = dedup_by_classname(cases, &mut tally);
    let index = RepoIndex::new(merged, input);

    // One batch for every case's items.
    let mut items: Vec<(String, &'static str)> = Vec::new();
    let mut per_case: Vec<CaseItems> = Vec::with_capacity(cases.len());
    for case in &cases {
        let mut ci = CaseItems::default();
        if let (Some(file), Some(line)) = (case.file.as_deref(), case.line) {
            ci.file_line = Some(items.len());
            items.push((format!("File \"{file}\", line {line}"), "stacktrace"));
        }
        for token in qname_tokens(case) {
            ci.tokens.push((token.clone(), items.len()));
            items.push((token, "test"));
        }
        if let Some(trace) = case.trace.as_deref().filter(|t| !t.trim().is_empty()) {
            ci.trace = Some(items.len());
            items.push((trace.to_string(), "stacktrace"));
        }
        per_case.push(ci);
    }
    let batch: Vec<(&str, &str)> = items.iter().map(|(t, k)| (t.as_str(), *k)).collect();
    let resolved = if batch.is_empty() { Vec::new() } else { merged.resolve_signals(&batch) };
    let first_hit = |item: usize| resolved.get(item).and_then(|ids| ids.first()).copied();

    let mut plan: BTreeMap<u64, Vec<Value>> = BTreeMap::new();
    // (test node, name) -> the source that reported it: a log copy of a
    // JUnit case with no classname folds here, once both mapped.
    let mut seen_tests: BTreeMap<(u64, &str), &str> = BTreeMap::new();
    for (case, ci) in cases.iter().zip(&per_case) {
        let mapped = map_test(merged, &index, case, ci, &first_hit);
        if let Some((node, _)) = mapped {
            match seen_tests.get(&(node.0, case.name.as_str())) {
                Some(&source) if source != case.source => {
                    tally.deduplicated += 1;
                    continue;
                }
                Some(_) => {}
                None => {
                    seen_tests.insert((node.0, case.name.as_str()), case.source.as_str());
                }
            }
        }
        tally.cases += 1;
        let (source, source_redacted) = redact_untrusted(&case.source);
        let (report, report_redacted) = redact_untrusted(&case.report);
        let redacted = case.redacted || source_redacted + report_redacted + run.map_or(0, |(_, n)| n) > 0;
        let test = match &case.classname {
            Some(c) => format!("{c}::{}", case.name),
            None => case.name.clone(),
        };
        let id = format!("{}:{test}", run.map_or("latest", |(r, _)| r));
        let base = |id: String| {
            let mut m = Map::new();
            m.insert("source".into(), Value::from(source.clone()));
            m.insert("id".into(), Value::from(id));
            if let Some((r, _)) = run {
                m.insert("run".into(), Value::from(r));
            }
            m.insert("report".into(), Value::from(report.clone()));
            m.insert("test".into(), Value::from(test.clone()));
            m.insert("status".into(), Value::from(case.status.clone()));
            if let Some(message) = &case.message {
                m.insert("message".into(), Value::from(message.clone()));
            }
            if redacted {
                m.insert("redacted".into(), Value::Bool(true));
            }
            m
        };

        let test_node = match mapped {
            Some((node, via)) => {
                match via {
                    Via::FileLine => tally.file_line += 1,
                    Via::Qname => tally.qname += 1,
                    Via::Name => tally.name += 1,
                }
                let mut e = base(id.clone());
                e.insert("role".into(), Value::from("test"));
                e.insert("via".into(), Value::from(via.as_str()));
                plan.entry(node.0).or_default().push(Value::Object(e));
                Some(node)
            }
            None => {
                tally.unmapped += 1;
                None
            }
        };

        let Some(frames) = ci.trace.and_then(|i| resolved.get(i)) else { continue };
        let test_file = test_node.and_then(|t| index.facts.get(&t)).and_then(|f| f.file.as_deref());
        let mut kept = 0usize;
        for (frame, &node) in frames.iter().enumerate() {
            if kept == MAX_IMPLICATED {
                break;
            }
            let Some(f) = index.facts.get(&node) else { continue };
            if Some(node) == test_node || f.fixture || (test_file.is_some() && f.file.as_deref() == test_file) {
                continue;
            }
            let mut e = base(format!("{id}#{frame}"));
            e.insert("role".into(), Value::from("implicated"));
            e.insert("frame".into(), Value::from(frame));
            plan.entry(node.0).or_default().push(Value::Object(e));
            tally.implicated += 1;
            kept += 1;
        }
    }
    (plan, tally)
}

/// The ladder: `file_line`, then `qname`, then `name` (see the module doc).
fn map_test(
    merged: &MergedGraph,
    index: &RepoIndex<'_>,
    case: &TestCaseRecord,
    ci: &CaseItems,
    first_hit: &impl Fn(usize) -> Option<NodeId>,
) -> Option<(NodeId, Via)> {
    // 1. file_line.
    if let (Some(item), Some(file)) = (ci.file_line, case.file.as_deref())
        && let Some(hit) = first_hit(item)
        && index.facts.get(&hit).is_some_and(|f| f.kind.is_some_and(|k| TEST_KINDS.contains(&k)))
        && index.file_is(hit, file)
    {
        return Some((hit, Via::FileLine));
    }
    // 2. qname: the longest suffix whose hit really is that qname (resolve
    // falls back to the bare name, which is no qname match). The hit may be
    // another repo's node in a merged build: the candidates are this repo's.
    for (token, item) in &ci.tokens {
        let Some(hit) = first_hit(*item) else { continue };
        let suffix = format!("::{token}");
        let names_token = merged
            .graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&hit))
            .is_some_and(|q| q == token || q.ends_with(&suffix));
        if !names_token {
            continue;
        }
        let tail = token.rsplit("::").next().unwrap_or(token);
        let pool = index.by_tail.get(tail).map_or(&[][..], Vec::as_slice);
        let qname_of = |id: &NodeId| index.facts.get(id).map_or("", |f| f.qname);
        let exact: Vec<NodeId> = pool.iter().copied().filter(|id| qname_of(id) == token).collect();
        let mut candidates: Vec<NodeId> = if exact.is_empty() {
            pool.iter().copied().filter(|id| qname_of(id).ends_with(&suffix)).collect()
        } else {
            exact
        };
        if candidates.is_empty() {
            continue;
        }
        if candidates.len() > 1
            && let Some(file) = case.file.as_deref()
        {
            let in_file: Vec<NodeId> = candidates.iter().copied().filter(|&id| index.file_is(id, file)).collect();
            if !in_file.is_empty() {
                candidates = in_file;
            }
        }
        return merged.pick_primary(&candidates).map(|id| (id, Via::Qname));
    }
    // 3. name: only an unambiguous FUNCTION / METHOD.
    let bare = base_name(case.name.rsplit("::").next().unwrap_or(&case.name));
    match index.tests_by_name.get(bare).map(Vec::as_slice) {
        Some([only]) => Some((*only, Via::Name)),
        _ => None,
    }
}

/// The `test` items of a case, longest first: every suffix of at least two
/// segments of its classname + name path. Empty when a segment holds
/// whitespace (a jest description is no qname).
fn qname_tokens(case: &TestCaseRecord) -> Vec<String> {
    let mut segs: Vec<&str> = case
        .classname
        .as_deref()
        .unwrap_or("")
        .split(['.', ':', '/', '\\'])
        .filter(|s| !s.is_empty())
        .collect();
    if segs.len() > 1 && segs.last().is_some_and(|s| SOURCE_EXTENSIONS.contains(s)) {
        segs.pop();
    }
    let name: Vec<&str> = case.name.split("::").filter(|s| !s.is_empty()).collect();
    let Some((last, head)) = name.split_last() else {
        return Vec::new();
    };
    segs.extend(head);
    segs.push(base_name(last));
    if segs.iter().any(|s| s.is_empty() || s.chars().any(char::is_whitespace)) {
        return Vec::new();
    }
    (0..segs.len().saturating_sub(1)).map(|k| segs[k..].join("::")).collect()
}

/// A test name without its parameter suffix (`test_x[a-1]`, `testGet(String)`)
/// or Go subtest (`TestPay/refund`); the name itself when the cut would
/// leave nothing.
fn base_name(name: &str) -> &str {
    match name.find(['[', '(', '/']) {
        Some(cut) if cut > 0 => &name[..cut],
        _ => name,
    }
}

/// `cases` with one copy per `(classname, name)`, the JUnit copy first,
/// then JUnit cases before log cases, each in snapshot order.
fn dedup_by_classname<'c>(cases: &'c [TestCaseRecord], tally: &mut Tally) -> Vec<&'c TestCaseRecord> {
    let mut kept: Vec<&TestCaseRecord> = Vec::new();
    let mut at: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for case in cases {
        let key = (case.classname.as_deref().unwrap_or(""), case.name.as_str());
        match at.get(&key) {
            Some(&i) => {
                tally.deduplicated += 1;
                if kept[i].source != SOURCE_JUNIT && case.source == SOURCE_JUNIT {
                    kept[i] = case;
                }
            }
            None => {
                at.insert(key, kept.len());
                kept.push(case);
            }
        }
    }
    // Stable: JUnit before log keeps the richer copy when two map to one test.
    kept.sort_by_key(|c| c.source != SOURCE_JUNIT);
    kept
}

/// Merge each node's planned entries into its FAIL cell (first copy of the
/// node in graph order), capped at [`MAX_ENTRIES_PER_NODE`].
fn write_fail_cells(merged: &mut MergedGraph, input: &RepoInputs, plan: BTreeMap<u64, Vec<Value>>, tally: &mut Tally) {
    if plan.is_empty() {
        return;
    }
    let mut done: BTreeSet<u64> = BTreeSet::new();
    for g in merged.graphs.iter_mut().filter(|g| g.repo == input.repo) {
        for n in g.nodes.iter_mut() {
            let Some(entries) = plan.get(&n.id.0) else { continue };
            if !done.insert(n.id.0) {
                continue;
            }
            let slot = n.cells.iter().position(|c| c.kind == cell_type::FAIL);
            let mut payload = slot.map(|i| n.cells[i].payload.clone());
            let mut merged_ok = true;
            for e in entries {
                match merge_entry(payload.as_ref(), e) {
                    Ok(p) => payload = Some(p),
                    Err(_) => {
                        // A FAIL cell that is not an entry array was never
                        // this stage's: left as is, the entries counted.
                        merged_ok = false;
                        break;
                    }
                }
            }
            let Some(payload) = payload.filter(|_| merged_ok) else {
                tally.dropped += entries.len();
                continue;
            };
            let (payload, dropped) = cap_entries(payload);
            tally.dropped += dropped;
            let cell = Cell { kind: cell_type::FAIL, payload };
            match slot {
                Some(i) => n.cells[i] = cell,
                None => n.cells.push(cell),
            }
            tally.fail_cells += 1;
        }
    }
}

/// Keep the first [`MAX_ENTRIES_PER_NODE`] entries of a sorted entry array
/// (the lowest `(source, id)`); returns the payload and how many were cut.
fn cap_entries(payload: CellPayload) -> (CellPayload, usize) {
    let CellPayload::Json(s) = &payload else {
        return (payload, 0);
    };
    let Ok(Value::Array(mut entries)) = serde_json::from_str::<Value>(s) else {
        return (payload, 0);
    };
    if entries.len() <= MAX_ENTRIES_PER_NODE {
        return (payload, 0);
    }
    let dropped = entries.len() - MAX_ENTRIES_PER_NODE;
    entries.truncate(MAX_ENTRIES_PER_NODE);
    // merge_entry already stored the keys sorted; a Value re-serialises them
    // in the order it parsed them.
    let text = serde_json::to_string(&Value::Array(entries)).unwrap_or_else(|_| String::from("[]"));
    (CellPayload::Json(text), dropped)
}

/// The file of a node's first POSITION cell.
fn position_file(n: &Node) -> Option<String> {
    let c = n.cells.iter().find(|c| c.kind == cell_type::POSITION)?;
    let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
        return None;
    };
    let v: Value = serde_json::from_str(s).ok()?;
    v.get("file")?.as_str().filter(|f| !f.is_empty()).map(str::to_string)
}

/// ORIGIN provenance `test_fixture` (`passes::tag_synthetic_provenance`).
fn is_fixture(n: &Node) -> bool {
    n.cells.iter().filter(|c| c.kind == cell_type::ORIGIN).any(|c| match &c.payload {
        CellPayload::Json(s) => serde_json::from_str::<Value>(s)
            .ok()
            .is_some_and(|v| v.get("provenance").and_then(Value::as_str) == Some("test_fixture")),
        _ => false,
    })
}

/// True when a POSITION file and a report's file name the same file: one is a
/// suffix of the other cut on a `/` (a report may give an absolute or a
/// deeper-rooted path). The report side is normalised (`\` -> `/`, no
/// leading `./`); POSITION paths are already repo-relative with `/`.
fn same_file(position: &str, reported: &str) -> bool {
    let reported = reported.trim().replace('\\', "/");
    let reported = reported.trim_start_matches("./");
    fn suffix_of(long: &str, short: &str) -> bool {
        !short.is_empty()
            && long.ends_with(short)
            && (long.len() == short.len() || long.as_bytes()[long.len() - short.len() - 1] == b'/')
    }
    suffix_of(position, reported) || suffix_of(reported, position)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(classname: Option<&str>, name: &str) -> TestCaseRecord {
        TestCaseRecord {
            source: SOURCE_JUNIT.into(),
            report: "r.xml".into(),
            suite: None,
            classname: classname.map(str::to_string),
            name: name.into(),
            file: None,
            line: None,
            status: "failed".into(),
            message: None,
            trace: None,
            redacted: false,
        }
    }

    #[test]
    fn tokens_are_the_suffixes_longest_first() {
        assert_eq!(
            qname_tokens(&case(Some("com.example.UserServiceTest"), "testGetUser")),
            ["com::example::UserServiceTest::testGetUser", "example::UserServiceTest::testGetUser", "UserServiceTest::testGetUser"]
        );
        // A trailing source extension is a file name, not a segment.
        assert_eq!(qname_tokens(&case(Some("tests/test_app.py"), "test_x[a-1]")), ["tests::test_app::test_x", "test_app::test_x"]);
        // A Rust module path in the name; a Go subtest suffix.
        assert_eq!(qname_tokens(&case(Some("mycrate"), "tests::it_works")), ["mycrate::tests::it_works", "tests::it_works"]);
        assert_eq!(qname_tokens(&case(Some("github.com/acme/pay"), "TestPay/refund")).last().map(String::as_str), Some("pay::TestPay"));
        // One segment is no qname; a description is no qname.
        assert!(qname_tokens(&case(None, "test_x")).is_empty());
        assert!(qname_tokens(&case(Some("Cart"), "adds an item")).is_empty());
    }

    #[test]
    fn base_name_cuts_parameters_only_after_a_name() {
        assert_eq!(base_name("test_x[1]"), "test_x");
        assert_eq!(base_name("testGet(String)"), "testGet");
        assert_eq!(base_name("TestPay/refund"), "TestPay");
        assert_eq!(base_name("[weird]"), "[weird]");
    }

    #[test]
    fn junit_copy_wins_the_dedup() {
        let mut log = case(Some("tests.test_app"), "test_x");
        log.source = "log".into();
        log.report = "ci.log".into();
        let junit = case(Some("tests.test_app"), "test_x");
        let other = case(Some("tests.test_app"), "test_y");
        let cases = [log, junit.clone(), other.clone()];
        let mut tally = Tally::default();
        let kept = dedup_by_classname(&cases, &mut tally);
        assert_eq!(kept, [&junit, &other]);
        assert_eq!(tally.deduplicated, 1);
    }

    #[test]
    fn same_file_is_a_boundary_suffix() {
        assert!(same_file("tests/test_app.py", "tests/test_app.py"));
        assert!(same_file("tests/test_app.py", "/ci/work/repo/tests/test_app.py"));
        assert!(same_file("src/test/java/UserServiceTest.java", "UserServiceTest.java"));
        assert!(same_file("tests/test_app.py", ".\\tests\\test_app.py"));
        assert!(!same_file("other/test_app.py", "tests/test_app.py"));
        assert!(!same_file("tests/my_test_app.py", "test_app.py"));
    }

    #[test]
    fn cap_keeps_the_lowest_ids() {
        let mut payload: Option<CellPayload> = None;
        for i in (0..25).rev() {
            let e = serde_json::json!({"source": "junit", "id": format!("latest:t{i:02}")});
            payload = Some(merge_entry(payload.as_ref(), &e).unwrap());
        }
        let (capped, dropped) = cap_entries(payload.unwrap());
        assert_eq!(dropped, 5);
        let CellPayload::Json(s) = capped else { panic!("json") };
        let v: Vec<Value> = serde_json::from_str(&s).unwrap();
        assert_eq!(v.len(), MAX_ENTRIES_PER_NODE);
        assert_eq!(v[0]["id"], "latest:t00");
        assert_eq!(v[19]["id"], "latest:t19");
    }
}
