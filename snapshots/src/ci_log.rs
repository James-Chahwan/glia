//! CI-log failure lines (LF.6a): a run with no JUnit XML still prints the
//! summary lines every runner does. Line shapes, no regex:
//!
//! - pytest `FAILED <path>::<name>[ - <msg>]` (and `ERROR <path>::<name>` for
//!   an errored case): `file` is the path, `classname` the dotted module plus
//!   any class (`tests.test_app.TestUser`, pytest's own JUnit classname);
//! - go `--- FAIL: <Name> (<secs>)`: the lines indented under it are its
//!   trace, and the package's later `FAIL\t<pkg>\t…` line sets `classname` for
//!   every case since the previous package line;
//! - cargo `test <a::b::c> ... FAILED`: `name` is the last `::` segment and
//!   `classname` the rest (a doctest `test src/lib.rs - f (line 3) ... FAILED`
//!   gives `file` and `line` instead); its `---- <a::b::c> stdout ----` block is
//!   the trace;
//! - jest `● <describe> › … › <name>`: `suite` is the describe path, `name` the
//!   test; the indented block under it is the trace, and the preceding
//!   `FAIL <path>` line is its `file`.
//!
//! ANSI colour codes and a leading GitHub Actions timestamp are stripped
//! first. A case printed twice (jest's closing summary repeats every failure)
//! is kept once. Every record is source `log` and is sanitized.

use std::borrow::Cow;
use std::collections::HashMap;

use glia_code_domain::snapshots::{SOURCE_LOG, STATUS_ERROR, STATUS_FAILED, TestCaseRecord};

/// Trace lines are kept up to this many bytes per case (the stored trace is
/// capped at `TRACE_CAP` chars, at most 16 KiB of UTF-8).
const TRACE_READ_CAP: usize = 64 * 1024;

/// (file, suite, classname, name, status): a case printed twice is one case.
type CaseKey = (Option<String>, Option<String>, Option<String>, String, String);

/// Parse the failure lines of one CI log into records (source `log`,
/// `report` as given).
pub fn parse_ci_log(text: &str, report: &str) -> Vec<TestCaseRecord> {
    let mut parser = LogParser { report, ..LogParser::default() };
    for raw in text.lines() {
        parser.line(&clean_line(raw));
    }
    parser.finish()
}

#[derive(Default)]
enum Capture {
    #[default]
    None,
    /// Lines indented deeper than the header's `indent` (blank lines too, for jest).
    Indented { rec: usize, indent: usize, keep_blank: bool },
    /// A cargo `---- name stdout ----` block, up to the next block or `failures:`.
    Cargo { rec: usize },
}

#[derive(Default)]
struct LogParser<'a> {
    report: &'a str,
    records: Vec<TestCaseRecord>,
    traces: Vec<(Vec<String>, usize)>,
    capture: Capture,
    go_pending: Vec<usize>,
    jest_file: Option<String>,
    cargo_by_name: HashMap<String, usize>,
}

impl LogParser<'_> {
    fn line(&mut self, line: &str) {
        if self.continue_capture(line) {
            return;
        }
        let trimmed = line.trim();
        let indent = indent_of(line);
        if let Some(rest) = trimmed.strip_prefix("FAILED ") {
            self.pytest(rest, STATUS_FAILED);
        } else if let Some(rest) = trimmed.strip_prefix("ERROR ") {
            self.pytest(rest, STATUS_ERROR);
        } else if let Some(rest) = trimmed.strip_prefix("--- FAIL: ") {
            let name = rest.split(" (").next().unwrap_or(rest).trim();
            let rec = self.push(None, None, name, STATUS_FAILED);
            self.go_pending.push(rec);
            self.capture = Capture::Indented { rec, indent, keep_blank: false };
        } else if let Some(rest) = line.strip_prefix("FAIL\t") {
            let pkg = rest.split(['\t', ' ']).next().unwrap_or("").trim();
            for rec in std::mem::take(&mut self.go_pending) {
                if !pkg.is_empty() {
                    self.records[rec].classname = Some(pkg.to_string());
                }
            }
        } else if line.starts_with("ok ") && line.contains('\t') {
            self.go_pending.clear();
        } else if let Some(full) = trimmed.strip_prefix("test ").and_then(|r| r.strip_suffix(" ... FAILED")) {
            self.cargo_record(full.trim());
        } else if let Some(full) = trimmed
            .strip_prefix("---- ")
            .and_then(|r| r.strip_suffix(" stdout ----").or_else(|| r.strip_suffix(" stderr ----")))
        {
            let rec = self.cargo_record(full.trim());
            self.capture = Capture::Cargo { rec };
        } else if let Some(title) = trimmed.strip_prefix("● ") {
            self.jest(title.trim(), indent);
        } else if let Some(path) = trimmed.strip_prefix("FAIL ") {
            let path = path.trim();
            self.jest_file = (!path.is_empty() && !path.contains(char::is_whitespace)).then(|| path.to_string());
        } else if trimmed.starts_with("PASS ") {
            self.jest_file = None;
        }
    }

    /// Feed `line` to the open trace; false (and the capture closed) when it
    /// does not belong there.
    fn continue_capture(&mut self, line: &str) -> bool {
        let t = line.trim();
        let keep = match self.capture {
            Capture::None => return false,
            Capture::Indented { indent, keep_blank, .. } => {
                let header = t.starts_with("--- ") || t.starts_with("=== ") || t.starts_with("● ");
                if t.is_empty() { keep_blank } else { indent_of(line) > indent && !header }
            }
            Capture::Cargo { .. } => {
                !(t == "failures:" || t.starts_with("test result:") || (t.starts_with("---- ") && t.ends_with(" ----")))
            }
        };
        let rec = match self.capture {
            Capture::Indented { rec, .. } | Capture::Cargo { rec } => rec,
            Capture::None => return false,
        };
        if !keep {
            self.capture = Capture::None;
            return false;
        }
        let (lines, bytes) = &mut self.traces[rec];
        if *bytes < TRACE_READ_CAP {
            *bytes += line.len() + 1;
            lines.push(line.to_string());
        }
        true
    }

    fn pytest(&mut self, rest: &str, status: &'static str) {
        let (id, msg) = match rest.split_once(" - ") {
            Some((id, msg)) => (id.trim(), Some(msg.trim())),
            None => (rest.trim(), None),
        };
        if !id.contains("::") {
            return; // `ERROR tests/x.py - ImportError`: a collection error names no test.
        }
        let (base, param) = id.find('[').map_or((id, ""), |k| (&id[..k], &id[k..]));
        let parts: Vec<&str> = base.split("::").collect();
        let (Some((last, classes)), Some(path)) = (parts[1..].split_last(), parts.first()) else { return };
        let module = path.strip_suffix(".py").unwrap_or(path).replace(['/', '\\'], ".");
        let classname = std::iter::once(module.as_str()).chain(classes.iter().copied()).collect::<Vec<_>>().join(".");
        let rec = self.push(Some(path.to_string()), Some(classname), &format!("{last}{param}"), status);
        self.records[rec].message = msg.filter(|m| !m.is_empty()).map(str::to_string);
    }

    /// The record for cargo test `full`, created on first sight.
    fn cargo_record(&mut self, full: &str) -> usize {
        if let Some(&rec) = self.cargo_by_name.get(full) {
            return rec;
        }
        // Doctest: `src/lib.rs - Foo::bar (line 12)`.
        let doctest = full.split_once(" - ").and_then(|(file, item)| {
            let k = item.rfind(" (line ")?;
            let line = item[k + 7..].strip_suffix(')')?.parse::<u32>().ok()?;
            Some((file, &item[..k], line))
        });
        let rec = match doctest {
            Some((file, item, line)) => {
                let rec = self.push(None, None, item, STATUS_FAILED);
                self.records[rec].file = Some(file.to_string());
                self.records[rec].line = Some(line);
                rec
            }
            None => match full.rsplit_once("::") {
                Some((path, name)) => self.push(None, Some(path.to_string()), name, STATUS_FAILED),
                None => self.push(None, None, full, STATUS_FAILED),
            },
        };
        self.cargo_by_name.insert(full.to_string(), rec);
        rec
    }

    fn jest(&mut self, title: &str, indent: usize) {
        if title.is_empty() || title == "Console" {
            return;
        }
        let segments: Vec<&str> = title.split(" › ").map(str::trim).collect();
        let (suite, name) = match segments.split_last() {
            Some((name, describe)) if !describe.is_empty() => (Some(describe.join(" › ")), *name),
            _ => (None, title),
        };
        let status = if title == "Test suite failed to run" { STATUS_ERROR } else { STATUS_FAILED };
        let rec = self.push(self.jest_file.clone(), None, name, status);
        self.records[rec].suite = suite;
        self.capture = Capture::Indented { rec, indent, keep_blank: true };
    }

    fn push(&mut self, file: Option<String>, classname: Option<String>, name: &str, status: &str) -> usize {
        self.records.push(TestCaseRecord {
            // `append_tests_run` stamps the run's seq.
            seq: 0,
            source: SOURCE_LOG.to_string(),
            report: self.report.to_string(),
            suite: None,
            classname,
            name: name.to_string(),
            file,
            line: None,
            status: status.to_string(),
            message: None,
            trace: None,
            redacted: false,
        });
        self.traces.push((Vec::new(), 0));
        self.records.len() - 1
    }

    /// Attach traces, derive missing messages, drop repeats, sanitize.
    fn finish(self) -> Vec<TestCaseRecord> {
        let mut out: Vec<TestCaseRecord> = Vec::new();
        let mut seen: HashMap<CaseKey, usize> = HashMap::new();
        for (mut record, (lines, _)) in self.records.into_iter().zip(self.traces) {
            let trace = dedent(&lines);
            if record.message.is_none() {
                record.message = trace_message(&trace);
            }
            record.trace = (!trace.is_empty()).then_some(trace);
            let key = (
                record.file.clone(),
                record.suite.clone(),
                record.classname.clone(),
                record.name.clone(),
                record.status.clone(),
            );
            match seen.get(&key) {
                Some(&first) => {
                    let kept = &mut out[first];
                    if kept.message.is_none() {
                        kept.message = record.message;
                    }
                    if kept.trace.is_none() {
                        kept.trace = record.trace;
                    }
                }
                None => {
                    seen.insert(key, out.len());
                    out.push(record);
                }
            }
        }
        for record in &mut out {
            record.sanitize();
        }
        out
    }
}

/// The message a trace implies: its first non-blank line, or for a Rust panic
/// (`thread '…' panicked at src/lib.rs:10:5:`) the line after it.
fn trace_message(trace: &str) -> Option<String> {
    let mut lines = trace.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next()?;
    let message = if first.contains("panicked at") { lines.next().unwrap_or(first) } else { first };
    Some(message.to_string())
}

/// Leading spaces and tabs.
fn indent_of(line: &str) -> usize {
    line.bytes().take_while(|b| *b == b' ' || *b == b'\t').count()
}

/// Join `lines` with their common indentation removed, blank edges trimmed.
fn dedent(lines: &[String]) -> String {
    let common = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| indent_of(l)).min().unwrap_or(0);
    let body: Vec<&str> = lines.iter().map(|l| l.get(common..).unwrap_or(l.trim_start()).trim_end()).collect();
    body.join("\n").trim_matches('\n').to_string()
}

/// `raw` without ANSI escape sequences or a leading GitHub Actions timestamp.
fn clean_line(raw: &str) -> Cow<'_, str> {
    let mut line: Cow<str> = if raw.contains('\x1b') {
        let mut out = String::with_capacity(raw.len());
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\x1b' {
                out.push(c);
            } else if chars.next_if_eq(&'[').is_some() {
                // CSI: parameters and intermediates, then one final byte in @..~.
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            } else {
                chars.next();
            }
        }
        Cow::Owned(out)
    } else {
        Cow::Borrowed(raw)
    };
    if let Some(rest) = strip_actions_timestamp(&line) {
        line = Cow::Owned(rest.to_string());
    }
    line
}

/// `2026-01-02T03:04:05.1234567Z <rest>` -> `<rest>`.
fn strip_actions_timestamp(line: &str) -> Option<&str> {
    let b = line.as_bytes();
    let date_ok = b.len() > 20
        && b[..4].iter().all(u8::is_ascii_digit)
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T';
    if !date_ok {
        return None;
    }
    let z = line[..line.len().min(40)].find("Z ")?;
    line[11..z].bytes().all(|c| c.is_ascii_digit() || c == b':' || c == b'.').then(|| &line[z + 2..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_and_actions_timestamps_are_stripped() {
        let log = "2026-09-19T06:00:00.1234567Z \x1b[31mFAILED\x1b[0m tests/test_a.py::test_x - boom\n";
        let rows = parse_ci_log(log, "ci.log");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "test_x");
        assert_eq!(rows[0].message.as_deref(), Some("boom"));
    }

    #[test]
    fn pytest_parametrized_ids_and_collection_errors() {
        let log = "FAILED tests/test_a.py::TestX::test_p[a::b-1] - AssertionError\n\
                   ERROR tests/test_b.py - ImportError: no module\n\
                   ERROR tests/test_c.py::test_fixture - RuntimeError\n";
        let rows = parse_ci_log(log, "ci.log");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].classname.as_deref(), Some("tests.test_a.TestX"));
        assert_eq!(rows[0].name, "test_p[a::b-1]");
        assert_eq!(rows[1].status, STATUS_ERROR);
        assert_eq!(rows[1].file.as_deref(), Some("tests/test_c.py"));
    }

    #[test]
    fn a_repeated_jest_failure_is_kept_once() {
        let log = "FAIL src/a.test.ts\n  ● A › works\n\n    boom\n\nSummary of all failing tests\n\
                   FAIL src/a.test.ts\n  ● A › works\n\n    boom\n";
        let rows = parse_ci_log(log, "ci.log");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].trace.as_deref(), Some("boom"));
    }
}
