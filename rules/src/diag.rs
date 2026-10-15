//! Spans, diagnostics and [`RulesError`]: the one error type every module of the
//! crate returns, and the located text a rule author reads.
//!
//! A [`Span`] is a byte range in one source plus the 1-based line and column of its
//! start. Offsets count BYTES and columns count CHARS, so a column is for display
//! only: text is never sliced by column (UTF-8). A [`Diagnostic`] names its source,
//! its position and, once the parser has read a clause's head, the predicate of the
//! rule it sits in as `name/arity`; it renders as
//! `<source>:<line>:<col>: error: <message> (in the rule for `p/2`)`, with no suffix
//! for a lexical diagnostic (no predicate).
//!
//! [`RulesError`]'s Display is the text the engine's `check` / `query` surfaces print
//! verbatim, so every variant's wording is pinned by tests here and in
//! rules/tests/errors.rs (DA.16): a wording change moves both. The crate has no
//! external dependency, so Display and `std::error::Error` are written by hand.

use std::fmt;

use crate::ir::SourceId;

/// A located range in one rule source: `start..end` are byte offsets, `line` and
/// `col` are the 1-based line and char column of `start`. `Span::default()` (all
/// zero) is the span of programmatically built IR.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub source: SourceId,
    pub start: u32,
    pub end: u32,
    pub line: u32,
    pub col: u32,
}

/// How bad a diagnostic is. An `Error` stops the program; a `Warning` rides along
/// with a successful analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        })
    }
}

/// One located message about a rule program.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Diagnostic {
    pub severity: Severity,
    pub span: Span,
    /// The name of the source the span points into (`Program::source_name`).
    pub source_name: String,
    /// The rule the diagnostic sits in, as `name/arity`; `None` before a clause's
    /// head is known (every lexical diagnostic).
    pub predicate: Option<String>,
    pub message: String,
}

impl Diagnostic {
    /// An error at `span` of `source_name`, in the rule for `predicate` when known.
    pub fn error(
        span: Span,
        source_name: &str,
        predicate: Option<&str>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(Severity::Error, span, source_name, predicate, message.into())
    }

    /// A warning at `span` of `source_name`, in the rule for `predicate` when known.
    pub fn warning(
        span: Span,
        source_name: &str,
        predicate: Option<&str>,
        message: impl Into<String>,
    ) -> Self {
        Self::new(Severity::Warning, span, source_name, predicate, message.into())
    }

    fn new(
        severity: Severity,
        span: Span,
        source_name: &str,
        predicate: Option<&str>,
        message: String,
    ) -> Self {
        Diagnostic {
            severity,
            span,
            source_name: source_name.to_string(),
            predicate: predicate.map(str::to_string),
            message,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}: {}",
            self.source_name, self.span.line, self.span.col, self.severity, self.message
        )?;
        match &self.predicate {
            Some(p) => write!(f, " (in the rule for `{p}`)"),
            None => Ok(()),
        }
    }
}

/// Every failure of the rules crate: a program that does not lex / parse, one that
/// fails analysis, an evaluation that hit its row budget, a builtin that failed, a
/// value of the wrong type at run time, or a request for an output the program
/// does not define.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum RulesError {
    /// Lexical or syntax errors, one diagnostic each.
    Parse(Vec<Diagnostic>),
    /// Analysis errors (names, arity, safety, stratification, aggregates).
    Invalid(Vec<Diagnostic>),
    /// Evaluation stopped: `predicate` (`name/arity`, its first rule at
    /// `source:line`) had derived `rows` rows against the budget `cap`.
    Budget { predicate: String, source: String, line: u32, rows: usize, cap: usize },
    /// A builtin (`name/arity`) returned an error.
    Builtin { name: String, message: String },
    /// A run-time type error; `message` names the predicate.
    Type { predicate: String, message: String },
    /// An output was asked for that the program does not define; `known` lists the
    /// predicates it does define, as `name/arity`.
    Unknown { predicate: String, known: Vec<String> },
}

impl RulesError {
    /// The diagnostics of a `Parse` or `Invalid` error, in the order they were
    /// recorded; empty for every other variant.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        match self {
            RulesError::Parse(d) | RulesError::Invalid(d) => d,
            _ => &[],
        }
    }
}

/// The variable names the budget hint writes into its seeded example.
const HINT_VARS: [&str; 4] = ["x", "y", "z", "w"];

/// The seeded-recursion example head for `name/arity`: `tc/2` -> `tc(x, y)`. A
/// predicate string without a parsable `/arity` reads as two arguments.
fn hint_head(predicate: &str) -> String {
    let (name, arity) = match predicate.rsplit_once('/') {
        Some((n, a)) => match a.parse::<usize>() {
            Ok(a) => (n, a),
            Err(_) => (predicate, 2),
        },
        None => (predicate, 2),
    };
    if arity == 0 {
        return name.to_string();
    }
    let vars: Vec<String> = (0..arity)
        .map(|i| match HINT_VARS.get(i) {
            Some(v) => (*v).to_string(),
            None => format!("x{}", i + 1),
        })
        .collect();
    format!("{name}({})", vars.join(", "))
}

impl fmt::Display for RulesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RulesError::Parse(diags) | RulesError::Invalid(diags) => {
                // Source order: by source, then byte offset; a stable sort keeps the
                // recorded order of two diagnostics at one position.
                let mut sorted: Vec<&Diagnostic> = diags.iter().collect();
                sorted.sort_by_key(|d| (d.span.source, d.span.start, d.span.end));
                for (i, d) in sorted.iter().enumerate() {
                    if i > 0 {
                        f.write_str("\n")?;
                    }
                    write!(f, "{d}")?;
                }
                Ok(())
            }
            RulesError::Budget { predicate, source, line, rows, cap } => {
                write!(f, "rule evaluation stopped: `{predicate}` ({source}:{line}) ")?;
                if rows == cap {
                    write!(f, "reached {rows} derived rows, the row budget.")?;
                } else {
                    write!(f, "reached {rows} derived rows against a row budget of {cap}.")?;
                }
                write!(
                    f,
                    " Its recursion is probably unseeded: bind an argument from a small \
                     relation (`{} :- seed(x), ...`), use reaches(x, y, \"CATS\") for \
                     reachability, or raise the budget.",
                    hint_head(predicate)
                )
            }
            RulesError::Builtin { name, message } => {
                write!(f, "builtin `{name}` failed: {message}")
            }
            RulesError::Type { message, .. } => f.write_str(message),
            RulesError::Unknown { predicate, known } => {
                let mut known: Vec<&str> = known.iter().map(String::as_str).collect();
                known.sort_unstable();
                if known.is_empty() {
                    write!(f, "unknown output `{predicate}`; the program defines no predicate")
                } else {
                    write!(
                        f,
                        "unknown output `{predicate}`; the program defines {}",
                        known.join(", ")
                    )
                }
            }
        }
    }
}

impl std::error::Error for RulesError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: u32, col: u32, start: u32) -> Span {
        Span { source: 0, start, end: start + 1, line, col }
    }

    #[test]
    fn diagnostic_display() {
        let d = Diagnostic::error(
            at(3, 14, 40),
            "rules",
            Some("violation/1"),
            "unknown edge category `CALS`",
        );
        assert_eq!(
            d.to_string(),
            "rules:3:14: error: unknown edge category `CALS` (in the rule for `violation/1`)"
        );
        let w = Diagnostic::warning(at(1, 1, 0), "rules", Some("p/2"), "singleton variable `z`");
        assert_eq!(
            w.to_string(),
            "rules:1:1: warning: singleton variable `z` (in the rule for `p/2`)"
        );
        assert!(Severity::Error < Severity::Warning);
    }

    #[test]
    fn budget_error_text() {
        let e = RulesError::Budget {
            predicate: "tc/2".into(),
            source: "rules".into(),
            line: 3,
            rows: 5_000_000,
            cap: 5_000_000,
        };
        assert_eq!(
            e.to_string(),
            "rule evaluation stopped: `tc/2` (rules:3) reached 5000000 derived rows, the row \
             budget. Its recursion is probably unseeded: bind an argument from a small relation \
             (`tc(x, y) :- seed(x), ...`), use reaches(x, y, \"CATS\") for reachability, or raise \
             the budget."
        );
        // The example head follows the predicate's name and arity; an overshoot names
        // both counts.
        let over = RulesError::Budget {
            predicate: "path/3".into(),
            source: "check".into(),
            line: 7,
            rows: 1_001,
            cap: 1_000,
        };
        assert_eq!(
            over.to_string(),
            "rule evaluation stopped: `path/3` (check:7) reached 1001 derived rows against a row \
             budget of 1000. Its recursion is probably unseeded: bind an argument from a small \
             relation (`path(x, y, z) :- seed(x), ...`), use reaches(x, y, \"CATS\") for \
             reachability, or raise the budget."
        );
        assert_eq!(hint_head("wide/6"), "wide(x, y, z, w, x5, x6)");
        assert_eq!(hint_head("flag/0"), "flag");
        assert_eq!(hint_head("bare"), "bare(x, y)");
        assert!(over.diagnostics().is_empty());
    }

    #[test]
    fn error_display_variants() {
        let b = RulesError::Builtin {
            name: "in_scope/2".into(),
            message: "scope `nowhere` holds no node".into(),
        };
        assert_eq!(b.to_string(), "builtin `in_scope/2` failed: scope `nowhere` holds no node");

        let u = RulesError::Unknown {
            predicate: "nope".into(),
            known: vec!["b/2".into(), "a/1".into()],
        };
        assert_eq!(u.to_string(), "unknown output `nope`; the program defines a/1, b/2");
        let none = RulesError::Unknown { predicate: "nope".into(), known: Vec::new() };
        assert_eq!(none.to_string(), "unknown output `nope`; the program defines no predicate");

        let t = RulesError::Type {
            predicate: "w/2".into(),
            message: "sum over a non-integer value in `w/2`".into(),
        };
        assert_eq!(t.to_string(), "sum over a non-integer value in `w/2`");

        let lexical = Diagnostic::error(at(1, 16, 15), "rules", None, "unterminated string");
        assert_eq!(lexical.to_string(), "rules:1:16: error: unterminated string");

        // Two diagnostics, recorded out of source order, render one per line in
        // source order; diagnostics() keeps the recorded order.
        let second =
            Diagnostic::error(at(4, 2, 60), "rules", Some("w/1"), "expected a term, found `)`");
        let first =
            Diagnostic::error(at(2, 1, 20), "rules", Some("v/1"), "unknown aggregate `avg`");
        let inv = RulesError::Invalid(vec![second.clone(), first.clone()]);
        assert_eq!(
            inv.to_string(),
            "rules:2:1: error: unknown aggregate `avg` (in the rule for `v/1`)\n\
             rules:4:2: error: expected a term, found `)` (in the rule for `w/1`)"
        );
        assert_eq!(inv.diagnostics(), &[second, first.clone()][..]);
        let parse = RulesError::Parse(vec![lexical.clone()]);
        assert_eq!(parse.to_string(), "rules:1:16: error: unterminated string");
        assert_eq!(parse.diagnostics(), &[lexical][..]);

        // It is a std error, so `?` lifts it into a Box<dyn Error>.
        let boxed: Box<dyn std::error::Error> = Box::new(b);
        assert!(boxed.to_string().starts_with("builtin `in_scope/2`"));
    }
}
