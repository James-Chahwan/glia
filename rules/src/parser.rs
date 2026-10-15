//! The Datalog surface parser: rule text into the IR (`ir::Program`), with a span on
//! every node and diagnostics that name the line, column and predicate.
//!
//! [`parse_program`] lexes the text with the shared lexer (`lexer::lex`; a lexical
//! error comes back as its `RulesError::Parse`, unchanged) and reads the tokens by
//! recursive descent over this grammar:
//!
//! ```text
//! program  := clause*
//! clause   := head ( ':-' body )? '.'
//! head     := ident ( '(' [ harg (',' harg)* ] ')' )?      // `ok.`, `ok() :- ...` and `ok :- ...` are arity 0
//! harg     := aggop '(' ident (',' ident)* ')' | term      // aggop = count|min|max|sum, only in a head
//! body     := literal (',' literal)*
//! literal  := ('!' | 'not') atom | atom | term cmpop term
//! atom     := ident '(' [ term (',' term)* ] ')'               // `p()` is a 0-arity atom
//! term     := ident | '_' | string | int | '-' int         // ident = variable
//! cmpop    := '=' | '!=' | '<>' | '<' | '<=' | '>' | '>='
//! ```
//!
//! **Variables and constants.** A bare identifier in argument position is a
//! variable and a constant is a quoted string or an integer: `node(r, "ROUTE")`. There
//! is no Prolog / Souffle capitalisation rule (`Route` is a variable too), on purpose:
//! rule authors are agents, and one spelling rule is less to trip on. A lone `_` is
//! the wildcard, which a fact may not contain. `not` before `(` is the atom `not`
//! (`not(x)`), before a comparison operator the variable `not`, and otherwise the
//! negation (`not p(x)`).
//!
//! **Literals.** A literal starting `ident (` is an atom; anything else is a
//! comparison `term cmpop term`. So a 0-arity atom takes parentheses in a body
//! (`done()`), while a 0-arity head may be written bare (`ok`) or `ok()`. An aggregate
//! (`count(x)`, `min`, `max`, `sum`) is a head argument only. `-` folds into the
//! integer after it, with an overflow check: `-9223372036854775808` is reachable,
//! `- -9223372036854775808` is out of range. The lexer reads `<-` as one arrow token,
//! so a comparison with a negative constant needs a space: `a < -1`.
//!
//! **Diagnostics.** Each error is located at the offending token (its line and char
//! column) and, once the clause's head is read, names the rule as `name/arity`:
//! `rules:2:1: error: expected `.` after the rule for `v/1`, found `w` (in the rule for
//! `v/1`)`. The texts are an interface (DA.16 and DB's `check` errors rows print them
//! verbatim). **Recovery:** after an error the parser skips to the clause's closing
//! `.` at bracket depth 0 and goes on, so one bad rule does not hide the next; it
//! stops after 20 diagnostics (`lexer::MAX_DIAGNOSTICS`). A `.` inside an open
//! bracket is skipped only when a `)` closing that bracket follows before the next
//! clause boundary, and a `:-` (never inside brackets) drops the open depth to 0, so an
//! unclosed `(` costs one clause, not the rest of the text.
//!
//! The parser never slices text by column: positions come from the lexer's spans,
//! which fall on char boundaries.

use crate::diag::{Diagnostic, RulesError, Span};
use crate::ir::{
    AggOp, Aggregate, Atom, CmpOp, Comparison, Head, HeadArg, Literal, Program, Rule, Term,
};
use crate::lexer::{self, MAX_DIAGNOSTICS, Token, TokenKind};

/// Parse `text`, the rule source named `source_name`, into a [`Program`] holding that
/// one source. Any lexical or syntax error returns `RulesError::Parse` with every
/// diagnostic found, at most 20, in source order.
pub fn parse_program(source_name: &str, text: &str) -> Result<Program, RulesError> {
    let mut program = Program::new();
    let source = program.add_source(source_name);
    let tokens = lexer::lex(source, source_name, text)?;
    let mut p = Parser { tokens: &tokens, pos: 0, predicate: None, open: 0 };
    let mut diags = Vec::new();
    while diags.len() < MAX_DIAGNOSTICS && p.peek().kind != TokenKind::Eof {
        match p.clause() {
            Ok(rule) => program.rules.push(rule),
            Err(fail) => {
                let pred = p.predicate.as_deref();
                diags.push(Diagnostic::error(fail.span, source_name, pred, fail.message));
                p.recover();
            }
        }
    }
    if diags.is_empty() { Ok(program) } else { Err(RulesError::Parse(diags)) }
}

/// A syntax error: where it is and what it says; the clause's predicate is added
/// when it is recorded.
struct Failure {
    span: Span,
    message: String,
}

type Parsed<T> = Result<T, Failure>;

/// What `peek` returns past the end; unreachable, since the lexer's tokens end `Eof`
/// and the parser never steps past it.
static END: Token =
    Token { kind: TokenKind::Eof, span: Span { source: 0, start: 0, end: 0, line: 0, col: 0 } };

/// The span from `from`'s start to `to`'s end.
fn join(from: Span, to: Span) -> Span {
    Span { end: to.end.max(from.start), ..from }
}

/// A token as a "found ..." message names it.
fn describe(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Eof => "end of input".to_string(),
        other => format!("`{other}`"),
    }
}

fn aggop(name: &str) -> Option<AggOp> {
    match name {
        "count" => Some(AggOp::Count),
        "min" => Some(AggOp::Min),
        "max" => Some(AggOp::Max),
        "sum" => Some(AggOp::Sum),
        _ => None,
    }
}

/// A comparison operator, or the `<-` the comparison reads as a misspelt `< -`.
fn is_cmpop(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Eq
            | TokenKind::Ne
            | TokenKind::Lt
            | TokenKind::Le
            | TokenKind::Gt
            | TokenKind::Ge
            | TokenKind::LeftArrow
    )
}

fn head_only(op: AggOp) -> String {
    format!("an aggregate ({op}) may appear only in a head")
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    /// The current clause's head as `name/arity`, once read.
    predicate: Option<String>,
    /// The `(` the current clause has opened and not closed.
    open: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &'a Token {
        self.tokens.get(self.pos).unwrap_or(&END)
    }

    fn peek_kind(&self, ahead: usize) -> &'a TokenKind {
        self.tokens.get(self.pos + ahead).map_or(&END.kind, |t| &t.kind)
    }

    /// Consume the next token (never the final `Eof`), tracking bracket depth.
    fn advance(&mut self) -> &'a Token {
        let t = self.peek();
        match t.kind {
            TokenKind::Eof => return t,
            TokenKind::LParen => self.open += 1,
            TokenKind::RParen => self.open = self.open.saturating_sub(1),
            _ => {}
        }
        self.pos += 1;
        t
    }

    fn eat(&mut self, kind: &TokenKind) -> Option<&'a Token> {
        if self.peek().kind == *kind { Some(self.advance()) } else { None }
    }

    /// An error at the next (unconsumed) token.
    fn fail<T>(&self, message: String) -> Parsed<T> {
        Err(Failure { span: self.peek().span, message })
    }

    fn found(&self) -> String {
        describe(&self.peek().kind)
    }

    fn clause(&mut self) -> Parsed<Rule> {
        self.predicate = None;
        self.open = 0;
        let head = self.head()?;
        let sig = head.signature();
        self.predicate = Some(sig.clone());
        let mut body = Vec::new();
        if self.eat(&TokenKind::ColonDash).is_some() {
            loop {
                body.push(self.literal()?);
                if self.eat(&TokenKind::Comma).is_none() {
                    break;
                }
            }
            if self.peek().kind != TokenKind::Dot {
                let hint = match self.peek().kind {
                    TokenKind::Semicolon => "; there is no `;` (or): write one rule per case",
                    _ => "",
                };
                let found = self.found();
                return self
                    .fail(format!("expected `.` after the rule for `{sig}`, found {found}{hint}"));
            }
        } else if self.peek().kind == TokenKind::Dot {
            let wild = head.args.iter().find_map(|a| match a {
                HeadArg::Term(Term::Wild(span)) => Some(*span),
                _ => None,
            });
            if let Some(span) = wild {
                return Err(Failure { span, message: "a fact may not contain `_`".to_string() });
            }
        } else {
            let hint = match self.peek().kind {
                TokenKind::LeftArrow => "; a rule is written `head :- body.`",
                _ => "",
            };
            let found = self.found();
            return self
                .fail(format!("expected `:-` or `.` after the head `{sig}`, found {found}{hint}"));
        }
        let dot = self.advance();
        Ok(Rule { span: join(head.span, dot.span), head, body })
    }

    fn head(&mut self) -> Parsed<Head> {
        let tok = self.peek();
        let pred = match &tok.kind {
            TokenKind::Ident(name) if name != "_" => name.clone(),
            TokenKind::ColonDash => {
                return self.fail(
                    "expected a rule head, found `:-`; a rule needs a head, as in `violation(x) :- ...`"
                        .to_string(),
                );
            }
            other => return self.fail(format!("expected a rule head, found {}", describe(other))),
        };
        self.advance();
        let mut span = tok.span;
        let mut args = Vec::new();
        if self.eat(&TokenKind::LParen).is_some() {
            let (items, close) = self.list(true, Self::head_arg)?;
            args = items;
            span = join(span, close);
        }
        Ok(Head { pred, args, span })
    }

    /// The items of a parenthesised list whose `(` was just consumed, and the span of
    /// its `)`.
    fn list<T>(
        &mut self,
        allow_empty: bool,
        item: fn(&mut Self) -> Parsed<T>,
    ) -> Parsed<(Vec<T>, Span)> {
        let mut items = Vec::new();
        if allow_empty && let Some(close) = self.eat(&TokenKind::RParen) {
            return Ok((items, close.span));
        }
        loop {
            items.push(item(self)?);
            match self.peek().kind {
                TokenKind::Comma => {
                    self.advance();
                }
                TokenKind::RParen => return Ok((items, self.advance().span)),
                _ => return self.fail(format!("expected `,` or `)`, found {}", self.found())),
            }
        }
    }

    fn head_arg(&mut self) -> Parsed<HeadArg> {
        let tok = self.peek();
        if let (TokenKind::Ident(name), TokenKind::LParen) = (&tok.kind, self.peek_kind(1)) {
            let Some(op) = aggop(name) else {
                return self.fail(format!("unknown aggregate `{name}`"));
            };
            self.advance();
            self.advance();
            let (vars, close) = self.list(false, Self::agg_var)?;
            return Ok(HeadArg::Agg(Aggregate { op, vars, span: join(tok.span, close) }));
        }
        self.term().map(HeadArg::Term)
    }

    fn agg_var(&mut self) -> Parsed<String> {
        match &self.peek().kind {
            TokenKind::Ident(name) if name != "_" => {
                self.advance();
                Ok(name.clone())
            }
            other => self.fail(format!("expected a variable, found {}", describe(other))),
        }
    }

    fn literal(&mut self) -> Parsed<Literal> {
        match (&self.peek().kind, self.peek_kind(1)) {
            (TokenKind::Bang, _) => {
                self.advance();
                self.atom(Some("!")).map(Literal::Neg)
            }
            (TokenKind::Ident(not), next)
                if not == "not" && *next != TokenKind::LParen && !is_cmpop(next) =>
            {
                self.advance();
                self.atom(Some("not")).map(Literal::Neg)
            }
            (TokenKind::Ident(_), TokenKind::LParen) => self.atom(None).map(Literal::Pos),
            (TokenKind::Ident(_) | TokenKind::Str(_) | TokenKind::Int(_) | TokenKind::Minus, _) => {
                self.comparison().map(Literal::Cmp)
            }
            (other, _) => self.fail(format!("expected a body literal, found {}", describe(other))),
        }
    }

    /// An atom; `negation` is the `!` / `not` just read, if any.
    fn atom(&mut self, negation: Option<&str>) -> Parsed<Atom> {
        let tok = self.peek();
        let pred = match (&tok.kind, negation) {
            (TokenKind::Ident(name), _) if name != "_" => name,
            (other, Some(neg)) => {
                return self
                    .fail(format!("expected an atom after `{neg}`, found {}", describe(other)));
            }
            (other, None) => {
                return self.fail(format!("expected a predicate name, found {}", describe(other)));
            }
        };
        if *self.peek_kind(1) != TokenKind::LParen {
            self.advance();
            return self.fail(format!(
                "expected `(` after `{pred}`, found {}; a body atom takes parentheses even at \
                 arity 0: write `{pred}()`",
                self.found()
            ));
        }
        if let Some(op) = aggop(pred) {
            return self.fail(head_only(op));
        }
        self.advance();
        self.advance();
        let (args, close) = self.list(true, Self::term)?;
        Ok(Atom { pred: pred.clone(), args, span: join(tok.span, close) })
    }

    fn comparison(&mut self) -> Parsed<Comparison> {
        let left = self.term()?;
        let op = match self.peek().kind {
            TokenKind::Eq => CmpOp::Eq,
            TokenKind::Ne => CmpOp::Ne,
            TokenKind::Lt => CmpOp::Lt,
            TokenKind::Le => CmpOp::Le,
            TokenKind::Gt => CmpOp::Gt,
            TokenKind::Ge => CmpOp::Ge,
            TokenKind::LeftArrow => {
                let hint = match self.peek_kind(1) {
                    TokenKind::Int(n) => format!("write `{left} < -{n}`"),
                    _ => format!("write `<` and `-` apart, as in `{left} < -1`"),
                };
                return self.fail(format!("`<-` here reads as an arrow; {hint}"));
            }
            _ => {
                let hint = match &left {
                    Term::Var { name, .. } => {
                        format!("; if `{name}` is a 0-arity atom, write `{name}()`")
                    }
                    _ => String::new(),
                };
                let found = self.found();
                return self.fail(format!(
                    "expected a comparison operator after `{left}`, found {found}{hint}"
                ));
            }
        };
        self.advance();
        let right = self.term()?;
        Ok(Comparison { op, span: join(left.span(), right.span()), left, right })
    }

    fn term(&mut self) -> Parsed<Term> {
        let tok = self.peek();
        let span = tok.span;
        let term = match &tok.kind {
            TokenKind::Ident(name) if *self.peek_kind(1) == TokenKind::LParen => {
                return match aggop(name) {
                    Some(op) => self.fail(head_only(op)),
                    None => {
                        self.fail(format!("expected a term, found `{name}(`; atoms do not nest"))
                    }
                };
            }
            TokenKind::Ident(name) if name == "_" => Term::Wild(span),
            TokenKind::Ident(name) => Term::Var { name: name.clone(), span },
            TokenKind::Str(value) => Term::Str { value: value.clone(), span },
            TokenKind::Int(value) => Term::Int { value: *value, span },
            TokenKind::Minus => {
                self.advance();
                let next = self.peek();
                let TokenKind::Int(value) = next.kind else {
                    return self
                        .fail(format!("expected an integer after `-`, found {}", self.found()));
                };
                self.advance();
                let span = join(span, next.span);
                return match value.checked_neg() {
                    Some(value) => Ok(Term::Int { value, span }),
                    None => Err(Failure { span, message: "integer out of range".to_string() }),
                };
            }
            other => return self.fail(format!("expected a term, found {}", describe(other))),
        };
        self.advance();
        Ok(term)
    }

    /// Skip to the end of the broken clause: past the next `.` at bracket depth 0, or
    /// to the end of the text. The skip starts at the offending token, at the clause's
    /// open depth.
    fn recover(&mut self) {
        let mut depth = self.open;
        loop {
            match self.peek().kind {
                TokenKind::Eof => return,
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth = depth.saturating_sub(1),
                TokenKind::ColonDash => depth = 0,
                TokenKind::Dot if depth == 0 || !self.dot_inside_brackets() => {
                    self.pos += 1;
                    return;
                }
                _ => {}
            }
            self.pos += 1;
        }
    }

    /// Does a `)` closing an already-open bracket follow the `.` at `pos` before the
    /// next clause boundary (a `.` at that depth, a `:-` or the end)?
    fn dot_inside_brackets(&self) -> bool {
        let mut depth = 0usize;
        for t in self.tokens.get(self.pos + 1..).unwrap_or_default() {
            match t.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen if depth == 0 => return true,
                TokenKind::RParen => depth -= 1,
                TokenKind::Dot if depth == 0 => return false,
                TokenKind::ColonDash | TokenKind::Eof => return false,
                _ => {}
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Program {
        match parse_program("rules", text) {
            Ok(p) => p,
            Err(e) => panic!("parse failed on {text:?}:\n{e}"),
        }
    }

    /// Every diagnostic's Display, in recorded order.
    fn errors(text: &str) -> Vec<String> {
        match parse_program("rules", text) {
            Ok(p) => panic!("parsed {text:?} without error:\n{p}"),
            Err(e) => e.diagnostics().iter().map(ToString::to_string).collect(),
        }
    }

    fn slice(text: &str, span: Span) -> &str {
        text.get(span.start as usize..span.end as usize).unwrap_or("<not a char boundary>")
    }

    fn strip_term(t: &Term) -> Term {
        let span = Span::default();
        match t {
            Term::Var { name, .. } => Term::Var { name: name.clone(), span },
            Term::Wild(_) => Term::Wild(span),
            Term::Str { value, .. } => Term::Str { value: value.clone(), span },
            Term::Int { value, .. } => Term::Int { value: *value, span },
        }
    }

    fn strip_atom(a: &Atom) -> Atom {
        Atom {
            pred: a.pred.clone(),
            args: a.args.iter().map(strip_term).collect(),
            span: Span::default(),
        }
    }

    /// The program with every span zeroed.
    fn strip(p: &Program) -> Program {
        let span = Span::default();
        let rules = p
            .rules
            .iter()
            .map(|r| Rule {
                head: Head {
                    pred: r.head.pred.clone(),
                    args: r
                        .head
                        .args
                        .iter()
                        .map(|a| match a {
                            HeadArg::Term(t) => HeadArg::Term(strip_term(t)),
                            HeadArg::Agg(g) => {
                                HeadArg::Agg(Aggregate { op: g.op, vars: g.vars.clone(), span })
                            }
                        })
                        .collect(),
                    span,
                },
                body: r
                    .body
                    .iter()
                    .map(|l| match l {
                        Literal::Pos(a) => Literal::Pos(strip_atom(a)),
                        Literal::Neg(a) => Literal::Neg(strip_atom(a)),
                        Literal::Cmp(c) => Literal::Cmp(Comparison {
                            op: c.op,
                            left: strip_term(&c.left),
                            right: strip_term(&c.right),
                            span,
                        }),
                    })
                    .collect(),
                span,
            })
            .collect();
        Program { sources: p.sources.clone(), rules }
    }

    const EVERY: &str = r#"// every construct, one per clause
allowed("web", 'api').
v(x) :- node(x, "ROUTE"), !handled(x).
w(x) :- node(x, _), not handled(x), x != "y", x <> "z".
d(x, n) :- depth(x, n), n >= -2, n < -9223372036854775808, n <= 3, 0 = 0, n > - 1.
ok :- p(1).
done() :- ok().
fan_in(y, count(x)) :- edge(x, y, "CALLS").
spread(a, min(n), max(n), sum(n, m)) :- q(a, n, m). /* block */
flag().
t(Route) :- r(Route), not(Route), "a\tb" = "q\"".
"#;

    #[test]
    fn parses_every_construct() {
        let p = parse(EVERY);
        let expected = r#"allowed("web", "api").
v(x) :- node(x, "ROUTE"), !handled(x).
w(x) :- node(x, _), !handled(x), x != "y", x != "z".
d(x, n) :- depth(x, n), n >= -2, n < -9223372036854775808, n <= 3, 0 = 0, n > -1.
ok :- p(1).
done :- ok().
fan_in(y, count(x)) :- edge(x, y, "CALLS").
spread(a, min(n), max(n), sum(n, m)) :- q(a, n, m).
flag.
t(Route) :- r(Route), not(Route), "a\tb" = "q\"".
"#;
        assert_eq!(p.to_string(), expected);
        assert_eq!(p.sources, vec!["rules"]);
        assert!(p.rules[0].is_fact());
        // `!` and `not` are one negation; `_` is the wildcard; `-` folds into the int.
        let w = &p.rules[2];
        assert!(matches!(&w.body[0], Literal::Pos(a) if matches!(a.args[1], Term::Wild(_))));
        assert!(matches!(&w.body[1], Literal::Neg(a) if a.pred == "handled"));
        assert!(matches!(&w.body[3], Literal::Cmp(c) if c.op == CmpOp::Ne));
        let d = &p.rules[3];
        let ints: Vec<i64> = d
            .body
            .iter()
            .filter_map(|l| match l {
                Literal::Cmp(Comparison { right: Term::Int { value, .. }, .. }) => Some(*value),
                _ => None,
            })
            .collect();
        assert_eq!(ints, [-2, i64::MIN, 3, 0, -1]);
        // A 0-arity head either way, a 0-arity body atom, aggregates in head order.
        assert_eq!(
            (p.rules[4].head.signature(), p.rules[5].head.signature()),
            ("ok/0".into(), "done/0".into())
        );
        assert!(matches!(&p.rules[5].body[0], Literal::Pos(a) if a.signature() == "ok/0"));
        let aggs: Vec<(AggOp, Vec<String>)> = p.rules[7]
            .head
            .args
            .iter()
            .filter_map(|a| match a {
                HeadArg::Agg(g) => Some((g.op, g.vars.clone())),
                HeadArg::Term(_) => None,
            })
            .collect();
        assert_eq!(
            aggs,
            [
                (AggOp::Min, vec!["n".to_string()]),
                (AggOp::Max, vec!["n".to_string()]),
                (AggOp::Sum, vec!["n".to_string(), "m".to_string()]),
            ]
        );
        assert_eq!(p.rules[8].head.signature(), "flag/0");
        // A capitalised identifier is a variable; `not(..)` is an atom named `not`.
        let t = &p.rules[9];
        assert!(
            matches!(&t.head.args[0], HeadArg::Term(Term::Var { name, .. }) if name == "Route")
        );
        assert!(matches!(&t.body[1], Literal::Pos(a) if a.pred == "not"));

        // No clause at all is an empty program that still names its source.
        let empty = parse_program("check", "  // nothing\n/* here */").unwrap();
        assert_eq!((empty.sources.clone(), empty.rules.len()), (vec!["check".to_string()], 0));
    }

    #[test]
    fn round_trip() {
        let texts = [
            EVERY,
            "fan(y, count(x)) :- edge(x, y, \"CALLS\").\nflag.\nok :- done(), 1 = 1.\n",
            "p(x) :- q(x, \"\\\\ \\\" \\n \\t \u{e9}\r\"), x < -9223372036854775808, x >= 9223372036854775807.",
            "not(x) :- p(x), !not(x), not q(x), count = 1, x = not, not != 2.",
        ];
        for text in texts {
            let first = parse(text);
            let rendered = first.to_string();
            let second = parse(&rendered);
            assert_eq!(strip(&second), strip(&first), "round trip of {text:?} via {rendered:?}");
            assert_eq!(second.to_string(), rendered);
        }
    }

    #[test]
    fn spans_point_at_tokens() {
        let text = "p(x) :- q(x).\n// a comment\nr(y) :- edge(y, _, \"CALLS\"), y != -3.\n\
                    f(y, count(x)) :- e(x, y).\nok() :- f(1, 2).\nok :- ok().\n";
        let p = parse(text);
        let r = &p.rules[1];
        let Literal::Pos(edge) = &r.body[0] else { panic!("{:?}", r.body[0]) };
        assert_eq!((edge.pred.as_str(), edge.span.line, edge.span.col), ("edge", 3, 9));
        assert_eq!(slice(text, edge.span), "edge(y, _, \"CALLS\")");
        assert_eq!((edge.args[1].span().line, edge.args[1].span().col), (3, 17));
        assert_eq!(slice(text, edge.args[2].span()), "\"CALLS\"");
        let Literal::Cmp(cmp) = &r.body[1] else { panic!("{:?}", r.body[1]) };
        assert_eq!(slice(text, cmp.span), "y != -3");
        assert_eq!(slice(text, cmp.right.span()), "-3");
        assert_eq!((cmp.span.line, cmp.span.col), (3, 30));
        assert_eq!(slice(text, r.head.span), "r(y)");
        assert_eq!(slice(text, r.span), "r(y) :- edge(y, _, \"CALLS\"), y != -3.");
        let HeadArg::Agg(count) = &p.rules[2].head.args[1] else { panic!("{:?}", p.rules[2].head) };
        assert_eq!((slice(text, count.span), count.span.line, count.span.col), ("count(x)", 4, 6));
        assert_eq!(slice(text, p.rules[3].head.span), "ok()");
        assert_eq!(slice(text, p.rules[4].head.span), "ok");
        assert_eq!(slice(text, p.rules[4].span), "ok :- ok().");
        assert!(p.rules.iter().all(|r| r.span.source == 0 && r.head.span.source == 0));
    }

    #[test]
    fn errors_name_line_col_and_predicate() {
        let err =
            parse_program("rules", "v(x) :- node(x, \"ROUTE\")\nw(y) :- edge(y, _, \"CALLS\").")
                .unwrap_err();
        assert_eq!(
            err.to_string(),
            "rules:2:1: error: expected `.` after the rule for `v/1`, found `w` (in the rule for `v/1`)"
        );
        let [d] = err.diagnostics() else { panic!("{err}") };
        assert_eq!((d.span.line, d.span.col, d.predicate.as_deref()), (2, 1, Some("v/1")));
        assert!(matches!(err, RulesError::Parse(_)));

        // Before the head is read there is no predicate to name.
        assert_eq!(errors("v(x, ) :- p(x)."), ["rules:1:6: error: expected a term, found `)`"]);
        assert_eq!(
            errors("v(x) :- p(x)"),
            [
                "rules:1:13: error: expected `.` after the rule for `v/1`, found end of input (in the rule for `v/1`)"
            ]
        );
        assert_eq!(errors("\"v\"(x)."), ["rules:1:1: error: expected a rule head, found `\"v\"`"]);
        // The source name is the caller's; a lexical error comes back as lex gives it.
        let check = parse_program("check", "ok.\np(\"abc).").unwrap_err();
        assert_eq!(check, lexer::lex(0, "check", "ok.\np(\"abc).").unwrap_err());
        assert_eq!(check.to_string(), "check:2:3: error: unterminated string");
    }

    #[test]
    fn recovery_collects_several() {
        let text = "a(x) :- p(x, ).\nok(x) :- p(x).\nb(x) :- count(x).\n\
                    fine(x) :- q(x), x != 1.\nc(x) :- p(x) q(x).\n";
        assert_eq!(
            errors(text),
            [
                "rules:1:14: error: expected a term, found `)` (in the rule for `a/1`)",
                "rules:3:9: error: an aggregate (count) may appear only in a head (in the rule for `b/1`)",
                "rules:5:14: error: expected `.` after the rule for `c/1`, found `q` (in the rule for `c/1`)",
            ]
        );
        // An unclosed `(` costs its own clause; a `.` inside an open bracket is
        // skipped only when the bracket closes before the next clause boundary. Each
        // later diagnostic shows the one before it resynchronised.
        let text = "p(x :- q(x).\nr(y) :- p(y, z.\nt(1) :- .\nu(a.b) :- t(a).\nw(2) :- !w.\n";
        assert_eq!(
            errors(text),
            [
                "rules:1:5: error: expected `,` or `)`, found `:-`",
                "rules:2:15: error: expected `,` or `)`, found `.` (in the rule for `r/1`)",
                "rules:3:9: error: expected a body literal, found `.` (in the rule for `t/1`)",
                "rules:4:4: error: expected `,` or `)`, found `.`",
                "rules:5:11: error: expected `(` after `w`, found `.`; a body atom takes parentheses \
                 even at arity 0: write `w()` (in the rule for `w/1`)",
            ]
        );
    }

    #[test]
    fn aggregate_outside_head() {
        assert_eq!(
            errors("v(x) :- count(x)."),
            [
                "rules:1:9: error: an aggregate (count) may appear only in a head (in the rule for `v/1`)"
            ]
        );
        assert_eq!(
            errors("v(x) :- p(x), x = sum(x).\nw(x) :- p(x), !min(x).\nu(x) :- p(max(x))."),
            [
                "rules:1:19: error: an aggregate (sum) may appear only in a head (in the rule for `v/1`)",
                "rules:2:16: error: an aggregate (min) may appear only in a head (in the rule for `w/1`)",
                "rules:3:11: error: an aggregate (max) may appear only in a head (in the rule for `u/1`)",
            ]
        );
        assert_eq!(
            errors("v(x, avg(y)) :- p(x, y)."),
            ["rules:1:6: error: unknown aggregate `avg`"]
        );
        assert_eq!(
            errors("f(count(*)) :- p(x).\ng(count()) :- p(x).\nh(x) :- p(q(x))."),
            [
                "rules:1:9: error: expected a variable, found `*`",
                "rules:2:9: error: expected a variable, found `)`",
                "rules:3:11: error: expected a term, found `q(`; atoms do not nest (in the rule for `h/1`)",
            ]
        );
    }

    #[test]
    fn left_arrow_hint() {
        assert_eq!(
            errors("v(a) :- p(a), a<-1."),
            [
                "rules:1:16: error: `<-` here reads as an arrow; write `a < -1` (in the rule for `v/1`)"
            ]
        );
        assert_eq!(
            errors("v(a) :- p(a), a<-b."),
            [
                "rules:1:16: error: `<-` here reads as an arrow; write `<` and `-` apart, as in `a < -1` (in the rule for `v/1`)"
            ]
        );
        assert_eq!(
            errors("p(x) <- q(x)."),
            [
                "rules:1:6: error: expected `:-` or `.` after the head `p/1`, found `<-`; a rule is written `head :- body.` (in the rule for `p/1`)"
            ]
        );
        // With the space it is a comparison.
        assert_eq!(parse("v(a) :- p(a), a < -1.").to_string(), "v(a) :- p(a), a < -1.\n");
    }

    #[test]
    fn twenty_cap() {
        let errs = errors(&"v(x) :- count(x).\n".repeat(30));
        assert_eq!(errs.len(), MAX_DIAGNOSTICS);
        assert_eq!(errs.len(), 20);
        assert!(errs[19].starts_with("rules:20:9: error: an aggregate (count)"), "{}", errs[19]);
    }

    #[test]
    fn integers_and_wildcards() {
        let p = parse("n(x) :- p(x), x > -9223372036854775808, x < - 5, x != 9223372036854775807.");
        assert_eq!(
            p.to_string(),
            "n(x) :- p(x), x > -9223372036854775808, x < -5, x != 9223372036854775807.\n"
        );
        assert_eq!(
            errors("n(x) :- p(x), x > - -9223372036854775808.\nm(x) :- p(x), x = - y."),
            [
                "rules:1:19: error: integer out of range (in the rule for `n/1`)",
                "rules:2:21: error: expected an integer after `-`, found `y` (in the rule for `m/1`)",
            ]
        );
        // A fact may not hold `_`; in a rule head it parses (analysis rejects it).
        assert_eq!(
            errors("allowed(\"web\", _).\nok(1).\nbad(_, _)."),
            [
                "rules:1:16: error: a fact may not contain `_` (in the rule for `allowed/2`)",
                "rules:3:5: error: a fact may not contain `_` (in the rule for `bad/2`)",
            ]
        );
        assert!(matches!(
            &parse("w(_) :- p(x).").rules[0].head.args[0],
            HeadArg::Term(Term::Wild(_))
        ));
    }

    #[test]
    fn bare_names_and_other_hints() {
        assert_eq!(
            errors("v :- !done.\nw :- p(), done.\nu :- not 3."),
            [
                "rules:1:11: error: expected `(` after `done`, found `.`; a body atom takes \
                 parentheses even at arity 0: write `done()` (in the rule for `v/0`)",
                "rules:2:15: error: expected a comparison operator after `done`, found `.`; if \
                 `done` is a 0-arity atom, write `done()` (in the rule for `w/0`)",
                "rules:3:10: error: expected an atom after `not`, found `3` (in the rule for `u/0`)",
            ]
        );
        assert_eq!(
            errors(":- p(x).\nv(x) :- p(x); q(x).\n_(x) :- p(x).\nw :- ! 1.\ny :- $p."),
            [
                "rules:1:1: error: expected a rule head, found `:-`; a rule needs a head, as in \
                 `violation(x) :- ...`",
                "rules:2:13: error: expected `.` after the rule for `v/1`, found `;`; there is no \
                 `;` (or): write one rule per case (in the rule for `v/1`)",
                "rules:3:1: error: expected a rule head, found `_`",
                "rules:4:8: error: expected an atom after `!`, found `1` (in the rule for `w/0`)",
                "rules:5:6: error: expected a body literal, found `$` (in the rule for `y/0`)",
            ]
        );
    }
}
