//! JUnit XML reports (LF.6a), the test runners' lingua franca: pytest
//! `--junitxml`, Maven Surefire, jest-junit, go-junit-report and most CI.
//!
//! Read with quick-xml's streaming `Reader`, never a DOM. The shapes read:
//! `<testsuites>` wrapping nested `<testsuite name>`s wrapping
//! `<testcase classname name file line>`, whose first `<failure>` or `<error>`
//! child (`message` attribute, body text or CDATA) makes it a failed or errored
//! case, else a `<skipped>` child makes it skipped, else it passed. Only failed
//! and errored cases become records; skipped and passed ones are counted.
//! `<system-out>`, `<system-err>` and `<properties>` are never read: captured
//! output is where a secret is likeliest, and nothing downstream needs it.
//!
//! pytest writes `line` 0-based (it is pytest's `item.location[1]`); the other
//! producers that write it (PHPUnit, xmlrunner) are 1-based. A case inside
//! pytest's suite — `<testsuite name="pytest">` (pytest's default
//! `junit_suite_name`) or the `<testsuites name="pytest tests">` root pytest 8+
//! writes — is shifted to 1-based. A pytest run with a custom suite name under
//! an older pytest keeps its 0-based line.

use std::borrow::Cow;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use glia_code_domain::snapshots::{SOURCE_JUNIT, STATUS_ERROR, STATUS_FAILED, TestCaseRecord};
use serde::Serialize;

use crate::MAX_REPORT_BYTES;

/// A failure body is read up to this many bytes. The stored trace is capped at
/// `TRACE_CAP` chars (at most 16 KiB of UTF-8), so nothing past this is kept.
const BODY_READ_CAP: usize = 64 * 1024;

/// Every test case one JUnit report holds, by outcome.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct JunitCounts {
    /// Every `<testcase>`: `failed + errors + skipped + passed`.
    pub cases: usize,
    pub failed: usize,
    pub errors: usize,
    pub skipped: usize,
    pub passed: usize,
}

/// What an open element is, so its end event knows what to close.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    Suite,
    Case,
    /// The outcome element whose text is the case's failure body.
    Outcome,
    Other,
}

struct Suite {
    name: Option<String>,
    pytest: bool,
}

struct Case {
    suite: Option<String>,
    classname: Option<String>,
    name: String,
    file: Option<String>,
    line: Option<u32>,
    outcome: Option<Outcome>,
    skipped: bool,
}

struct Outcome {
    status: &'static str,
    message: Option<String>,
    body: String,
}

#[derive(Default)]
struct State {
    open: Vec<Open>,
    suites: Vec<Suite>,
    case: Option<Case>,
    records: Vec<TestCaseRecord>,
    counts: JunitCounts,
    root_seen: bool,
}

/// Parse one JUnit XML report: its failed and errored cases as records
/// (source `junit`, `report` as given, each sanitized) and its per-outcome
/// counts. Malformed XML, a document cut off inside an element, a root other
/// than `<testsuites>` / `<testsuite>`, or more than 50 MiB is an error for
/// this report only.
pub fn parse_junit(bytes: &[u8], report: &str) -> Result<(Vec<TestCaseRecord>, JunitCounts), String> {
    if bytes.len() > MAX_REPORT_BYTES {
        return Err(format!("{} bytes is over the {} MiB report cap", bytes.len(), MAX_REPORT_BYTES >> 20));
    }
    let text = String::from_utf8_lossy(bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let mut reader = Reader::from_str(text);
    let mut st = State::default();
    loop {
        let event = reader
            .read_event()
            .map_err(|e| format!("malformed XML at byte {}: {e}", reader.error_position()))?;
        match event {
            Event::Start(e) => {
                let kind = st.start(&e)?;
                st.open.push(kind);
            }
            Event::Empty(e) => {
                let kind = st.start(&e)?;
                st.end(kind, report);
            }
            Event::End(_) => {
                let kind = st.open.pop().unwrap_or(Open::Other);
                st.end(kind, report);
            }
            Event::Text(t) if st.capturing() => {
                let text = t.unescape().unwrap_or_else(|_| Cow::Owned(String::from_utf8_lossy(&t).into_owned()));
                st.push_body(&text);
            }
            Event::CData(c) if st.capturing() => {
                st.push_body(&String::from_utf8_lossy(&c.into_inner()));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !st.open.is_empty() {
        return Err(format!("document ends with {} element(s) still open", st.open.len()));
    }
    if !st.root_seen {
        return Err("no <testsuites> or <testsuite> element".to_string());
    }
    Ok((st.records, st.counts))
}

impl State {
    /// Classify a start (or empty) element and apply what it opens.
    fn start(&mut self, e: &BytesStart) -> Result<Open, String> {
        let local = e.local_name();
        let tag = local.as_ref();
        if !self.root_seen {
            if tag != b"testsuites" && tag != b"testsuite" {
                return Err(format!("root element <{}> is not <testsuites> or <testsuite>", String::from_utf8_lossy(tag)));
            }
            self.root_seen = true;
        }
        let parent = self.open.last().copied();
        let kind = match tag {
            b"testsuites" => {
                let pytest = self.in_pytest() || attr(e, b"name").is_some_and(|n| n.starts_with("pytest"));
                self.suites.push(Suite { name: None, pytest });
                Open::Suite
            }
            b"testsuite" => {
                let name = attr(e, b"name");
                let pytest = self.in_pytest() || name.as_deref() == Some("pytest");
                self.suites.push(Suite { name, pytest });
                Open::Suite
            }
            b"testcase" if self.case.is_none() => {
                let pytest = self.in_pytest();
                let line = attr(e, b"line").and_then(|l| l.trim().parse::<u32>().ok());
                let line = if pytest { line.map(|l| l.saturating_add(1)) } else { line.filter(|&l| l > 0) };
                self.case = Some(Case {
                    suite: self.suites.iter().rev().find_map(|s| s.name.clone()),
                    classname: attr(e, b"classname").or_else(|| attr(e, b"class")),
                    name: attr(e, b"name").unwrap_or_default(),
                    file: attr(e, b"file"),
                    line,
                    outcome: None,
                    skipped: false,
                });
                Open::Case
            }
            b"failure" | b"error" if parent == Some(Open::Case) => match &mut self.case {
                Some(case) if case.outcome.is_none() => {
                    let status = if tag == b"failure" { STATUS_FAILED } else { STATUS_ERROR };
                    case.outcome = Some(Outcome { status, message: attr(e, b"message"), body: String::new() });
                    Open::Outcome
                }
                _ => Open::Other,
            },
            b"skipped" if parent == Some(Open::Case) => {
                if let Some(case) = &mut self.case {
                    case.skipped = true;
                }
                Open::Other
            }
            _ => Open::Other,
        };
        Ok(kind)
    }

    fn end(&mut self, kind: Open, report: &str) {
        match kind {
            Open::Suite => {
                self.suites.pop();
            }
            Open::Case => {
                if let Some(case) = self.case.take() {
                    self.finish(case, report);
                }
            }
            Open::Outcome | Open::Other => {}
        }
    }

    fn finish(&mut self, case: Case, report: &str) {
        self.counts.cases += 1;
        let Some(outcome) = case.outcome else {
            if case.skipped {
                self.counts.skipped += 1;
            } else {
                self.counts.passed += 1;
            }
            return;
        };
        if outcome.status == STATUS_FAILED {
            self.counts.failed += 1;
        } else {
            self.counts.errors += 1;
        }
        let body = outcome.body.trim();
        let message = outcome
            .message
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .or_else(|| body.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string));
        let mut record = TestCaseRecord {
            source: SOURCE_JUNIT.to_string(),
            report: report.to_string(),
            suite: case.suite,
            classname: case.classname,
            name: case.name,
            file: case.file,
            line: case.line,
            status: outcome.status.to_string(),
            message,
            trace: (!body.is_empty()).then(|| body.to_string()),
            redacted: false,
        };
        record.sanitize();
        self.records.push(record);
    }

    fn in_pytest(&self) -> bool {
        self.suites.last().is_some_and(|s| s.pytest)
    }

    /// Whether text events belong to the current case's failure body.
    fn capturing(&self) -> bool {
        self.open.last() == Some(&Open::Outcome)
    }

    fn push_body(&mut self, text: &str) {
        let Some(outcome) = self.case.as_mut().and_then(|c| c.outcome.as_mut()) else { return };
        let room = BODY_READ_CAP.saturating_sub(outcome.body.len());
        let mut cut = text.len().min(room);
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        outcome.body.push_str(&text[..cut]);
    }
}

/// An attribute's unescaped value, matched on its local name (`xsi:x` is `x`).
fn attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes().flatten().find(|a| a.key.local_name().as_ref() == key).map(|a| {
        a.unescape_value()
            .map(Cow::into_owned)
            .unwrap_or_else(|_| String::from_utf8_lossy(&a.value).into_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_suites_name_the_innermost_and_first_outcome_wins() {
        let xml = br#"<testsuites><testsuite name="outer"><testsuite name="inner">
            <testcase classname="c" name="t"><failure message="first">a</failure><error message="second">b</error></testcase>
          </testsuite><testcase name="u"><skipped/></testcase></testsuite></testsuites>"#;
        let (rows, counts) = parse_junit(xml, "r.xml").unwrap();
        assert_eq!(counts, JunitCounts { cases: 2, failed: 1, errors: 0, skipped: 1, passed: 0 });
        assert_eq!(rows[0].suite.as_deref(), Some("inner"));
        assert_eq!(rows[0].message.as_deref(), Some("first"));
        assert_eq!(rows[0].trace.as_deref(), Some("a"));
    }

    #[test]
    fn a_non_junit_document_is_an_error() {
        let err = parse_junit(b"<project><modelVersion>4</modelVersion></project>", "pom.xml").unwrap_err();
        assert!(err.contains("<project>"), "{err}");
        let err = parse_junit(b"<testsuite name=\"s\"><testcase name=\"t\">", "cut.xml").unwrap_err();
        assert!(err.contains("still open"), "{err}");
    }

    #[test]
    fn line_is_one_based_outside_pytest() {
        let xml = br#"<testsuite name="PHPUnit"><testcase name="t" file="a.php" line="12"><error/></testcase>
            <testcase name="z" line="0"><error/></testcase></testsuite>"#;
        let (rows, _) = parse_junit(xml, "r.xml").unwrap();
        assert_eq!(rows[0].line, Some(12));
        assert_eq!(rows[1].line, None, "0 is not a 1-based line");
    }
}
