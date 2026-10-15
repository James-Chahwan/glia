//! The GQL read subset's recursive-descent parser (Pratt for WHERE) over the shared
//! lexer's tokens (R1 §5: one tokenizer for both surfaces). [`parse`] reads
//!
//! ```text
//! query    := match+ [WHERE expr] RETURN [DISTINCT] item (',' item)* [ORDER BY key (',' key)*] [LIMIT int] [';']
//! match    := MATCH pattern (',' pattern)*
//! pattern  := [ident '=' [ANY SHORTEST]] node (rel node)*
//! node     := '(' [ident] [':' ident ('|' ident)*] ['{' ident ':' lit (',' ident ':' lit)* '}'] ')'
//! rel      := '-[' body ']->' [gquant] | '<-[' body ']-' [gquant] | '-[' body ']-' [gquant] | '-->' | '<--' | '--'
//! body     := [ident] [':' ident ('|' ident)*] ['*' [int] ['..' [int]]]
//! gquant   := '+' | '*' | '{' int ',' [int] '}' | '{' int '}'
//! expr     := OR < AND < NOT < cmp
//! cmp      := operand (('=' | '<>' | '!=' | '<' | '<=' | '>' | '>=') operand | STARTS WITH operand
//!             | ENDS WITH operand | CONTAINS operand | IN '[' lit (',' lit)* ']')
//!             | pattern | EXISTS '{' pattern '}' | ident '(' [operand (',' operand)*] ')' | '(' expr ')'
//! operand  := ident | ident '.' ident | lit | TYPE '(' ident ')'
//! item     := (operand | (COUNT | MIN | MAX | SUM) '(' ('*' | [DISTINCT] operand) ')') [AS ident]
//! key      := (ident | ident '.' ident) [ASC | DESC]
//! lit      := string | ['-'] int | TRUE | FALSE
//! ```
//!
//! Keywords arrive as identifiers and compare case-insensitively; identifiers keep
//! their case. A `(` in WHERE opens a pattern when `)`, `:` or `{` follows it
//! (after an optional variable), else a parenthesised condition. Quantifiers are
//! normalised (Cypher `*` is {1,}, `*n` {n,n}, `*m..` {m,}, `*..n` {1,n}; GQL `+`
//! {1,}, `*` {0,}); a lower bound above the upper one, a typed relationship of at
//! most 0 hops, and both spellings on one relationship are diagnostics.
//!
//! **Refused by name** (the subset is the contract, R1 §10), each at its keyword:
//! OPTIONAL MATCH, WITH, UNWIND, UNION, CALL, CREATE, MERGE, SET, DELETE, DETACH
//! DELETE, REMOVE, FOREACH, LOAD, SKIP (where a clause may start); CASE, `$`
//! parameters, shortestPath() / allShortestPaths() (where a value or a pattern may
//! start); ALL SHORTEST, and the restrictors TRAIL / ACYCLIC / SIMPLE (glia returns
//! endpoint pairs under walk semantics, Q4).
//!
//! **Recovery.** A clause that fails skips to the next clause keyword (bracket depth
//! zero), so one query reports every error, at most 20, in source order. A clause
//! out of order is reported and still read. A missing clause at the end of input is
//! reported only when nothing else was (it is usually the cascade of an earlier
//! error). Diagnostics are DA's, located in the source `<gql>`.
//!
//! fired_on: `[gql] parse clauses=<n> patterns=<p> diagnostics=<d>` on stderr once
//! per [`parse`]: clauses read (MATCH, WHERE, RETURN, ORDER BY, LIMIT), path
//! patterns read (MATCH and WHERE), diagnostics returned.

use crate::diag::{Diagnostic, RulesError, Span};
use crate::ir::{AggOp, CmpOp};
use crate::lexer::{self, MAX_DIAGNOSTICS, Token, TokenKind};

use super::ast::{
    Agg, Dir, Expr, Ident, Lit, MatchClause, NodePat, Operand, OrderKey, PathPattern, Quant, Query,
    RelPat, RetExpr, ReturnItem, StrOp,
};

/// The source name every GQL diagnostic renders (`<gql>:<line>:<col>: error: ..`).
pub const SOURCE_NAME: &str = "<gql>";

const SUBSET: &str = "is not in the GQL read subset (docs/rules.md)";
const SHAPE: &str =
    "a query reads MATCH .. [WHERE ..] RETURN [DISTINCT] .. [ORDER BY ..] [LIMIT n]";
const VALUE: &str = "a variable, `var.key`, a literal or type(r)";
/// Clause keywords outside the subset, refused where a clause may start.
const REFUSED_CLAUSES: [&str; 14] = [
    "OPTIONAL", "WITH", "UNWIND", "UNION", "CALL", "CREATE", "MERGE", "SET", "DELETE", "DETACH",
    "REMOVE", "FOREACH", "LOAD", "SKIP",
];
/// Operator and modifier words: with the clause keywords, never a variable or alias.
const OPERATOR_WORDS: [&str; 17] = [
    "AND", "OR", "NOT", "XOR", "AS", "DISTINCT", "IN", "STARTS", "ENDS", "CONTAINS", "EXISTS",
    "BY", "ASC", "DESC", "TRUE", "FALSE", "CASE",
];
const RESTRICTORS: [&str; 3] = ["TRAIL", "ACYCLIC", "SIMPLE"];
/// How deep a WHERE condition may nest before it is refused (no stack overflow).
const MAX_NESTING: usize = 64;

/// Parse one GQL read query. A lexical error comes back as the lexer's diagnostics
/// unchanged; otherwise every syntax error and refusal found, at most 20.
pub fn parse(text: &str) -> Result<Query, Vec<Diagnostic>> {
    let tokens = match lexer::lex(0, SOURCE_NAME, text) {
        Ok(tokens) => tokens,
        Err(RulesError::Parse(diags)) => {
            eprintln!("[gql] parse clauses=0 patterns=0 diagnostics={}", diags.len());
            return Err(diags);
        }
        Err(other) => {
            eprintln!("[gql] parse clauses=0 patterns=0 diagnostics=1");
            return Err(vec![Diagnostic::error(
                Span::default(),
                SOURCE_NAME,
                None,
                other.to_string(),
            )]);
        }
    };
    let eof_span = tokens.last().map(|t| t.span).unwrap_or_default();
    let mut p = Parser {
        tokens,
        eof: Token { kind: TokenKind::Eof, span: eof_span },
        pos: 0,
        diags: Vec::new(),
        clauses: 0,
        patterns: 0,
        nesting: 0,
    };
    let query = p.query();
    eprintln!(
        "[gql] parse clauses={} patterns={} diagnostics={}",
        p.clauses,
        p.patterns,
        p.diags.len()
    );
    if p.diags.is_empty() { Ok(query) } else { Err(p.diags) }
}

/// Where the query is in `match+ [WHERE] RETURN [ORDER BY] [LIMIT] [;]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Stage {
    Start,
    Match,
    Where,
    Return,
    Order,
    Limit,
    End,
}

impl Stage {
    /// What may come next, for an "expected .." message.
    fn expected(self) -> &'static str {
        match self {
            Stage::Start => "MATCH",
            Stage::Match => "`,`, MATCH, WHERE or RETURN",
            Stage::Where => "AND, OR or RETURN",
            Stage::Return => "`,`, ORDER BY, LIMIT or end of input",
            Stage::Order => "`,`, LIMIT or end of input",
            Stage::Limit => "end of input",
            Stage::End => "end of input after `;`",
        }
    }
}

/// The subset's clause keywords.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Clause {
    Match,
    Where,
    Return,
    Order,
    Limit,
}

impl Clause {
    fn of(word: &str) -> Option<Clause> {
        [
            ("MATCH", Clause::Match),
            ("WHERE", Clause::Where),
            ("RETURN", Clause::Return),
            ("ORDER", Clause::Order),
            ("LIMIT", Clause::Limit),
        ]
        .into_iter()
        .find(|(w, _)| word.eq_ignore_ascii_case(w))
        .map(|(_, c)| c)
    }

    /// The stage this clause moves to, and whether `from` may be followed by it.
    fn step(self, from: Stage) -> (Stage, bool) {
        match self {
            Clause::Match => (Stage::Match, from <= Stage::Match),
            Clause::Where => (Stage::Where, from == Stage::Match),
            Clause::Return => (Stage::Return, matches!(from, Stage::Match | Stage::Where)),
            Clause::Order => (Stage::Order, from == Stage::Return),
            Clause::Limit => (Stage::Limit, matches!(from, Stage::Return | Stage::Order)),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Clause::Match => "MATCH",
            Clause::Where => "WHERE",
            Clause::Return => "RETURN",
            Clause::Order => "ORDER BY",
            Clause::Limit => "LIMIT",
        }
    }
}

/// A clause keyword (subset or refused) or an operator word: never a variable or an
/// alias, so `WHERE a.x =` before `RETURN` reports the missing value. A label, a
/// relationship type and a property key may spell one (`(o:Order {limit: 1})`).
fn reserved(word: &str) -> bool {
    Clause::of(word).is_some()
        || REFUSED_CLAUSES.iter().chain(OPERATOR_WORDS.iter()).any(|k| word.eq_ignore_ascii_case(k))
}

fn agg_op(word: &str) -> Option<AggOp> {
    [AggOp::Count, AggOp::Min, AggOp::Max, AggOp::Sum]
        .into_iter()
        .find(|op| word.eq_ignore_ascii_case(op.name()))
}

/// The canonical name of a Cypher shortest-path function, refused by name.
fn shortest_fn(word: &str) -> Option<&'static str> {
    ["shortestPath", "allShortestPaths"].into_iter().find(|f| word.eq_ignore_ascii_case(f))
}

/// `a` .. `b` as one span (`a` starts it, `b` ends it).
fn join(a: Span, b: Span) -> Span {
    Span { end: b.end.max(a.end), ..a }
}

/// The `-` of a greedy `:-` / `<-` token (`pair`, two ASCII bytes) through `int`:
/// the span of the negative literal the hint reads it as.
fn minus_through(pair: Span, int: Span) -> Span {
    Span { start: pair.start + 1, col: pair.col + 1, ..join(pair, int) }
}

struct Parser {
    tokens: Vec<Token>,
    /// Returned past the end; the lexer always ends the stream with one.
    eof: Token,
    pos: usize,
    diags: Vec<Diagnostic>,
    clauses: usize,
    patterns: usize,
    nesting: usize,
}

impl Parser {
    // ---- token access ------------------------------------------------------

    fn tok(&self, ahead: usize) -> &Token {
        self.tokens.get(self.pos + ahead).unwrap_or(&self.eof)
    }

    fn kind(&self) -> &TokenKind {
        &self.tok(0).kind
    }

    fn at(&self, kind: &TokenKind) -> bool {
        self.kind() == kind
    }

    fn kw(&self, ahead: usize, word: &str) -> bool {
        matches!(&self.tok(ahead).kind, TokenKind::Ident(s) if s.eq_ignore_ascii_case(word))
    }

    /// The identifier at `ahead`, if it is one.
    fn word(&self, ahead: usize) -> Option<&str> {
        match &self.tok(ahead).kind {
            TokenKind::Ident(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Consume the current token, returning its span (end of input stays put).
    fn bump(&mut self) -> Span {
        let span = self.tok(0).span;
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        span
    }

    /// `start` .. the end of the last consumed token.
    fn span_from(&self, start: Span) -> Span {
        match self.pos.checked_sub(1).and_then(|i| self.tokens.get(i)) {
            Some(last) => join(start, last.span),
            None => start,
        }
    }

    fn previous(&self) -> Option<&TokenKind> {
        self.pos.checked_sub(1).and_then(|i| self.tokens.get(i)).map(|t| &t.kind)
    }

    // ---- diagnostics -------------------------------------------------------

    fn full(&self) -> bool {
        self.diags.len() >= MAX_DIAGNOSTICS
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        if !self.full() {
            self.diags.push(Diagnostic::error(span, SOURCE_NAME, None, message));
        }
    }

    /// Refuse `name` at `span`, with an optional hint after a colon.
    fn refuse(&mut self, span: Span, name: &str, hint: &str) {
        let tail = if hint.is_empty() { String::new() } else { format!(": {hint}") };
        self.error(span, format!("`{name}` {SUBSET}{tail}"));
    }

    /// "expected <what>, found <token>" at the current token; a `$` parameter is
    /// refused by name instead. Always `None`, for `return self.unexpected(..)`.
    fn unexpected<T>(&mut self, what: &str) -> Option<T> {
        let t = self.tok(0).clone();
        if t.kind == TokenKind::Dollar {
            let (name, span) = match &self.tok(1).kind {
                TokenKind::Ident(p) if self.tok(1).span.start == t.span.end => {
                    (format!("${p}"), join(t.span, self.tok(1).span))
                }
                _ => ("$".to_string(), t.span),
            };
            self.error(span, format!("parameter `{name}` {SUBSET}: write the value as a literal"));
        } else {
            let found = match &t.kind {
                TokenKind::Eof => "end of input".to_string(),
                k => format!("`{k}`"),
            };
            self.error(t.span, format!("expected {what}, found {found}"));
        }
        None
    }

    fn expect(&mut self, kind: TokenKind, what: &str) -> Option<Span> {
        if self.at(&kind) { Some(self.bump()) } else { self.unexpected(what) }
    }

    fn ident(&mut self, what: &str) -> Option<Ident> {
        match self.kind().clone() {
            TokenKind::Ident(name) => Some(Ident { name, span: self.bump() }),
            _ => self.unexpected(what),
        }
    }

    /// A variable or alias: an identifier that is not [`reserved`].
    fn var(&mut self, what: &str) -> Option<Ident> {
        if self.word(0).is_some_and(reserved) { self.unexpected(what) } else { self.ident(what) }
    }

    /// Is a variable (an unreserved identifier) at `ahead`?
    fn at_var(&self, ahead: usize) -> bool {
        self.word(ahead).is_some_and(|w| !reserved(w))
    }

    // ---- recovery ----------------------------------------------------------

    /// Is the current token a clause keyword, subset or refused? A keyword
    /// spelling right after `.`, `:`, `|` or `(` is a key or label, one after AS is
    /// the (refused) alias, and `WITH` after STARTS / ENDS is the string test's.
    fn at_clause(&self) -> bool {
        let Some(word) = self.word(0) else { return false };
        match self.previous() {
            Some(TokenKind::Dot | TokenKind::Colon | TokenKind::Pipe | TokenKind::LParen) => {
                return false;
            }
            Some(TokenKind::Ident(prev))
                if prev.eq_ignore_ascii_case("AS")
                    || (word.eq_ignore_ascii_case("WITH")
                        && (prev.eq_ignore_ascii_case("STARTS")
                            || prev.eq_ignore_ascii_case("ENDS"))) =>
            {
                return false;
            }
            _ => {}
        }
        Clause::of(word).is_some() || REFUSED_CLAUSES.iter().any(|k| word.eq_ignore_ascii_case(k))
    }

    /// Skip to the next clause keyword, `;` or the end, at bracket depth zero
    /// (counted from here, so a skip that starts inside brackets closes them).
    fn recover(&mut self) {
        let mut depth = 0usize;
        loop {
            match self.kind() {
                TokenKind::Eof => return,
                TokenKind::Semicolon if depth == 0 => return,
                _ if depth == 0 && self.at_clause() => return,
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    depth = depth.saturating_sub(1)
                }
                _ => {}
            }
            self.bump();
        }
    }

    // ---- clauses -----------------------------------------------------------

    fn query(&mut self) -> Query {
        let start = self.tok(0).span;
        let mut q = Query {
            matches: Vec::new(),
            condition: None,
            distinct: false,
            items: Vec::new(),
            order_by: Vec::new(),
            limit: None,
            span: start,
        };
        let mut stage = Stage::Start;
        let mut returned = false;
        while !self.full() && !self.at(&TokenKind::Eof) {
            if stage == Stage::End {
                self.unexpected::<()>(stage.expected());
                break;
            }
            if self.at(&TokenKind::Semicolon) && returned {
                self.bump();
                stage = Stage::End;
                continue;
            }
            match self.word(0).and_then(Clause::of) {
                Some(clause) => {
                    let (next, in_order) = clause.step(stage);
                    if !in_order {
                        let span = self.tok(0).span;
                        self.error(span, format!("`{}` is out of place: {SHAPE}", clause.name()));
                    }
                    stage = stage.max(next);
                    self.clauses += 1;
                    returned |= clause == Clause::Return;
                    self.clause(clause, &mut q);
                }
                None => {
                    let refused = self.refused_clause();
                    if refused.is_none() {
                        self.unexpected::<()>(stage.expected());
                        self.bump();
                    }
                    self.recover();
                    // A query that did not start with MATCH has had its error; what
                    // follows UNION is a second query, read for its own errors.
                    stage = if refused == Some("UNION") {
                        Stage::Match
                    } else {
                        stage.max(Stage::Match)
                    };
                }
            }
        }
        if !returned && self.diags.is_empty() {
            self.unexpected::<()>(stage.expected());
        }
        q.span = self.span_from(start);
        q
    }

    /// Refuse the clause keyword at the cursor, if it is one, consuming it (and the
    /// MATCH of OPTIONAL MATCH, the DELETE of DETACH DELETE).
    fn refused_clause(&mut self) -> Option<&'static str> {
        let word = self.word(0)?;
        let kw = REFUSED_CLAUSES.into_iter().find(|k| word.eq_ignore_ascii_case(k))?;
        let start = self.bump();
        let name = match kw {
            "OPTIONAL" if self.kw(0, "MATCH") => "OPTIONAL MATCH",
            "DETACH" if self.kw(0, "DELETE") => "DETACH DELETE",
            other => other,
        };
        if name.contains(' ') {
            self.bump();
        }
        let span = self.span_from(start);
        self.refuse(span, name, "");
        Some(kw)
    }

    /// Read one clause whose keyword is at the cursor; a failure recovers.
    fn clause(&mut self, clause: Clause, q: &mut Query) {
        let start = self.bump();
        match clause {
            Clause::Match => {
                let mut patterns = Vec::new();
                loop {
                    match self.path_pattern(true) {
                        Some(p) => patterns.push(p),
                        None => {
                            self.recover();
                            break;
                        }
                    }
                    if !self.at(&TokenKind::Comma) {
                        break;
                    }
                    self.bump();
                }
                let span = self.span_from(start);
                q.matches.push(MatchClause { patterns, span });
            }
            Clause::Where => match self.expr(0) {
                Some(e) => q.condition = Some(e),
                None => self.recover(),
            },
            Clause::Return => {
                if self.kw(0, "DISTINCT") {
                    self.bump();
                    q.distinct = true;
                }
                self.list(&mut q.items, Self::return_item);
            }
            Clause::Order => {
                if self.kw(0, "BY") {
                    self.bump();
                    self.list(&mut q.order_by, Self::order_key);
                } else {
                    self.unexpected::<()>("BY after ORDER");
                    self.recover();
                }
            }
            Clause::Limit => match self.kind() {
                TokenKind::Int(v) if *v >= 0 => {
                    q.limit = u64::try_from(*v).ok();
                    self.bump();
                }
                _ => {
                    self.unexpected::<()>("a non-negative integer after LIMIT");
                    self.recover();
                }
            },
        }
    }

    /// `x (',' x)*` into `out`; a failed element recovers and ends the list.
    fn list<T>(&mut self, out: &mut Vec<T>, mut element: impl FnMut(&mut Self) -> Option<T>) {
        loop {
            match element(self) {
                Some(x) => out.push(x),
                None => {
                    self.recover();
                    return;
                }
            }
            if !self.at(&TokenKind::Comma) {
                return;
            }
            self.bump();
        }
    }

    fn return_item(&mut self) -> Option<ReturnItem> {
        let start = self.tok(0).span;
        let op = self.word(0).and_then(agg_op);
        let expr = match op {
            Some(op) if self.tok(1).kind == TokenKind::LParen => RetExpr::Agg(self.agg(op)?),
            _ => RetExpr::Operand(self.operand()?),
        };
        let alias = if self.kw(0, "AS") {
            self.bump();
            Some(self.var("an alias after AS")?)
        } else {
            None
        };
        Some(ReturnItem { expr, alias, span: self.span_from(start) })
    }

    /// `count(*)`, `op([DISTINCT] operand)`; the cursor is on the name.
    fn agg(&mut self, op: AggOp) -> Option<Agg> {
        let start = self.bump();
        self.bump();
        let (distinct, arg) = if self.at(&TokenKind::Star) {
            let star = self.bump();
            if op != AggOp::Count {
                self.error(star, "`*` is allowed only in count(*)");
            }
            (false, None)
        } else {
            let distinct = self.kw(0, "DISTINCT");
            if distinct {
                self.bump();
            }
            (distinct, Some(self.operand()?))
        };
        self.expect(TokenKind::RParen, "`)` to close the aggregate")?;
        Some(Agg { op, distinct, arg, span: self.span_from(start) })
    }

    fn order_key(&mut self) -> Option<OrderKey> {
        let start = self.tok(0).span;
        if self.word(0).is_some() && self.tok(1).kind == TokenKind::LParen {
            self.error(
                start,
                "ORDER BY takes a column alias or `var.key`: alias the expression \
                 (`count(b) AS n`) and order by the alias",
            );
            return None;
        }
        let var = self.var("a column alias or `var.key`")?;
        let key = if self.at(&TokenKind::Dot) {
            self.bump();
            let key = self.ident("a property key after `.`")?;
            Operand::Prop { span: join(var.span, key.span), var, key }
        } else {
            Operand::Var(var)
        };
        let descending = self.kw(0, "DESC");
        if descending || self.kw(0, "ASC") {
            self.bump();
        }
        Some(OrderKey { key, descending, span: self.span_from(start) })
    }

    // ---- patterns ----------------------------------------------------------

    /// A path pattern; `top` (a MATCH pattern) may carry a path variable and the
    /// selector / restrictor words.
    fn path_pattern(&mut self, top: bool) -> Option<PathPattern> {
        let start = self.tok(0).span;
        let mut var = None;
        let mut shortest = false;
        if top {
            if self.at_var(0) && self.tok(1).kind == TokenKind::Eq {
                var = self.ident("a path variable");
                self.bump();
            }
            self.selector(&mut shortest, var.is_some());
            if let Some(r) = self
                .word(0)
                .and_then(|w| RESTRICTORS.into_iter().find(|r| w.eq_ignore_ascii_case(r)))
            {
                let span = self.bump();
                self.refuse(span, r, "glia returns endpoint pairs under walk semantics (Q4)");
            }
        }
        if let Some(f) = self.word(0).and_then(shortest_fn)
            && self.tok(1).kind == TokenKind::LParen
        {
            let span = self.tok(0).span;
            self.refuse(span, &format!("{f}()"), "write `p = ANY SHORTEST (a)-[:T]->+(b)`");
            return None;
        }
        let first = self.node()?;
        let mut steps = Vec::new();
        while matches!(self.kind(), TokenKind::Minus | TokenKind::LeftArrow) {
            let rel = self.rel()?;
            let node = self.node()?;
            steps.push((rel, node));
        }
        self.patterns += 1;
        Some(PathPattern { var, shortest, start: first, steps, span: self.span_from(start) })
    }

    /// `ANY SHORTEST` (the one selector), or a refused / incomplete one.
    fn selector(&mut self, shortest: &mut bool, has_var: bool) {
        if self.kw(0, "ANY") && self.kw(1, "SHORTEST") {
            let start = self.bump();
            self.bump();
            *shortest = true;
            if !has_var {
                let span = self.span_from(start);
                self.error(
                    span,
                    "`ANY SHORTEST` needs a path variable: `p = ANY SHORTEST (a)-[:T]->+(b)`",
                );
            }
        } else if self.kw(0, "ALL") {
            let start = self.bump();
            let name = if self.kw(0, "SHORTEST") {
                self.bump();
                "ALL SHORTEST"
            } else {
                "ALL"
            };
            let span = self.span_from(start);
            self.refuse(span, name, "");
        } else if self.kw(0, "ANY") {
            self.bump();
            self.unexpected::<()>("SHORTEST after ANY (the one path selector is ANY SHORTEST)");
        }
    }

    fn node(&mut self) -> Option<NodePat> {
        let start = self.tok(0).span;
        self.expect(TokenKind::LParen, "`(` to start a node pattern")?;
        let var = if self.at_var(0) { self.ident("a node variable") } else { None };
        let labels = self.names("a label after `:`")?;
        let mut props = Vec::new();
        if self.at(&TokenKind::LBrace) {
            self.bump();
            loop {
                props.push(self.prop()?);
                if !self.at(&TokenKind::Comma) {
                    break;
                }
                self.bump();
            }
            self.expect(TokenKind::RBrace, "`,` or `}` in the property map")?;
        }
        self.expect(TokenKind::RParen, "`)` to close the node pattern")?;
        Some(NodePat { var, labels, props, span: self.span_from(start) })
    }

    /// `[':' name ('|' name)*]`: labels or relationship types.
    fn names(&mut self, what: &str) -> Option<Vec<Ident>> {
        let mut names = Vec::new();
        if self.at(&TokenKind::Colon) {
            self.bump();
            names.push(self.ident(what)?);
            while self.at(&TokenKind::Pipe) {
                self.bump();
                names.push(self.ident(what)?);
            }
        }
        Some(names)
    }

    /// `key: lit`; `key:-1` (`:-` lexes as one token) gets the spacing hint.
    fn prop(&mut self) -> Option<(Ident, Lit)> {
        let key = self.ident("a property key")?;
        if self.at(&TokenKind::ColonDash) {
            let dash = self.bump();
            if let TokenKind::Int(v) = *self.kind() {
                let span = minus_through(dash, self.bump());
                self.error(
                    dash,
                    format!("`:-` reads as one token here: write `{}: -{v}`", key.name),
                );
                return Some((key, Lit::Int { value: v.checked_neg().unwrap_or(v), span }));
            }
            return self.unexpected("an integer after `:-`");
        }
        self.expect(TokenKind::Colon, "`:` after the property key")?;
        Some((key, self.lit()?))
    }

    fn rel(&mut self) -> Option<RelPat> {
        let start = self.tok(0).span;
        let left = self.at(&TokenKind::LeftArrow);
        self.bump();
        let bracketed = self.at(&TokenKind::LBracket);
        let (mut var, mut types, mut inner) = (None, Vec::new(), None);
        if bracketed {
            self.bump();
            if self.at_var(0) {
                var = self.ident("a relationship variable");
            }
            types = self.names("a relationship type after `:`")?;
            if self.at(&TokenKind::Star) {
                inner = Some(self.star_quant()?);
            }
            self.expect(TokenKind::RBracket, "`]` to close the relationship")?;
        }
        let dir = match (left, self.kind()) {
            (false, TokenKind::Arrow) => Dir::Out,
            (false, TokenKind::Minus) => Dir::Undirected,
            (true, TokenKind::Minus) => Dir::In,
            (true, _) => return self.unexpected("`-` to end the `<-` relationship"),
            (false, _) if bracketed => return self.unexpected("`->` or `-` after `]`"),
            (false, _) => return self.unexpected("`[`, `->` or `-` after `-`"),
        };
        self.bump();
        let mut quant = inner;
        if matches!(self.kind(), TokenKind::Plus | TokenKind::Star | TokenKind::LBrace) {
            let outer = self.gquant()?;
            if !bracketed {
                self.error(
                    outer.span,
                    "a quantifier follows a bracketed relationship: write `-[]->+`, not `-->+`",
                );
            } else if inner.is_some() {
                self.error(
                    outer.span,
                    "a relationship takes one quantifier, `*m..n` inside the brackets or \
                     `+` / `*` / `{m,n}` after them, not both",
                );
            } else {
                quant = Some(outer);
            }
        }
        if let Some(q) = quant {
            self.check_quant(q, &types);
        }
        Some(RelPat { var, types, dir, quant, span: self.span_from(start) })
    }

    /// Cypher `*`, `*n`, `*m..`, `*..n`, `*m..n` inside the brackets.
    fn star_quant(&mut self) -> Option<Quant> {
        let start = self.bump();
        let lo = if matches!(self.kind(), TokenKind::Int(_)) { Some(self.bound()?) } else { None };
        let (min, max) = if self.at(&TokenKind::DotDot) {
            self.bump();
            let hi =
                if matches!(self.kind(), TokenKind::Int(_)) { Some(self.bound()?) } else { None };
            (lo.unwrap_or(1), hi)
        } else {
            match lo {
                Some(n) => (n, Some(n)),
                None => (1, None),
            }
        };
        Some(Quant { min, max, span: self.span_from(start) })
    }

    /// GQL `+`, `*`, `{m,n}`, `{m,}`, `{m}` after the closing arrow.
    fn gquant(&mut self) -> Option<Quant> {
        let start = self.tok(0).span;
        let (min, max) = match self.kind() {
            TokenKind::Plus => {
                self.bump();
                (1, None)
            }
            TokenKind::Star => {
                self.bump();
                (0, None)
            }
            _ => {
                self.bump();
                let lo = self.bound()?;
                let hi = if self.at(&TokenKind::Comma) {
                    self.bump();
                    if matches!(self.kind(), TokenKind::Int(_)) {
                        Some(self.bound()?)
                    } else {
                        None
                    }
                } else {
                    Some(lo)
                };
                self.expect(TokenKind::RBrace, "`}` to close the quantifier")?;
                (lo, hi)
            }
        };
        Some(Quant { min, max, span: self.span_from(start) })
    }

    fn bound(&mut self) -> Option<u32> {
        match *self.kind() {
            TokenKind::Int(v) => {
                let span = self.bump();
                let bound = u32::try_from(v).ok();
                if bound.is_none() {
                    self.error(span, format!("quantifier bound {v} is out of range"));
                }
                bound
            }
            _ => self.unexpected("an integer bound"),
        }
    }

    fn check_quant(&mut self, q: Quant, types: &[Ident]) {
        match q.max {
            Some(max) if q.min > max => self.error(
                q.span,
                format!("the quantifier's lower bound {} exceeds its upper bound {max}", q.min),
            ),
            Some(0) if !types.is_empty() => {
                let names: Vec<&str> = types.iter().map(|t| t.name.as_str()).collect();
                self.error(
                    q.span,
                    format!(
                        "a quantifier of at most 0 hops never takes a `{}` relationship: drop \
                         the type or raise the upper bound",
                        names.join("|")
                    ),
                );
            }
            _ => {}
        }
    }

    // ---- WHERE -------------------------------------------------------------

    /// Pratt: OR (1) < AND (2) < NOT (prefix, 3) < cmp.
    fn expr(&mut self, min_bp: u8) -> Option<Expr> {
        self.nesting += 1;
        let e = if self.nesting > MAX_NESTING {
            let span = self.tok(0).span;
            self.error(span, format!("the condition nests deeper than {MAX_NESTING} levels"));
            None
        } else {
            self.expr_inner(min_bp)
        };
        self.nesting -= 1;
        e
    }

    fn expr_inner(&mut self, min_bp: u8) -> Option<Expr> {
        let mut left = if self.kw(0, "NOT") {
            let start = self.bump();
            let inner = self.expr(3)?;
            Expr::Not { span: join(start, inner.span()), expr: Box::new(inner) }
        } else {
            self.cmp()?
        };
        loop {
            let bp = if self.kw(0, "OR") {
                1
            } else if self.kw(0, "AND") {
                2
            } else {
                break;
            };
            if bp < min_bp {
                break;
            }
            self.bump();
            let right = self.expr(bp + 1)?;
            let span = join(left.span(), right.span());
            let (left_box, right_box) = (Box::new(left), Box::new(right));
            left = if bp == 1 {
                Expr::Or { left: left_box, right: right_box, span }
            } else {
                Expr::And { left: left_box, right: right_box, span }
            };
        }
        Some(left)
    }

    /// Does the `(` at the cursor open a pattern (`()`, `(:`, `({`, `(v)`, `(v:`,
    /// `(v{`) rather than a parenthesised condition?
    fn paren_is_pattern(&self) -> bool {
        let opens_node =
            |k: &TokenKind| matches!(k, TokenKind::RParen | TokenKind::Colon | TokenKind::LBrace);
        opens_node(&self.tok(1).kind) || (self.word(1).is_some() && opens_node(&self.tok(2).kind))
    }

    fn cmp(&mut self) -> Option<Expr> {
        if self.at(&TokenKind::LParen) {
            if self.paren_is_pattern() {
                let p = self.path_pattern(false)?;
                return Some(Expr::Pattern { span: p.span, pattern: Box::new(p), exists: false });
            }
            self.bump();
            let e = self.expr(0)?;
            self.expect(TokenKind::RParen, "`)` to close the condition")?;
            return Some(e);
        }
        if self.kw(0, "EXISTS") && self.tok(1).kind == TokenKind::LBrace {
            let start = self.bump();
            self.bump();
            let p = self.path_pattern(false)?;
            self.expect(TokenKind::RBrace, "`}` to close EXISTS")?;
            let span = self.span_from(start);
            return Some(Expr::Pattern { pattern: Box::new(p), exists: true, span });
        }
        if self.word(0).is_some_and(|w| !w.eq_ignore_ascii_case("type"))
            && self.tok(1).kind == TokenKind::LParen
        {
            return self.call();
        }
        let left = self.operand()?;
        self.comparison(left)
    }

    /// A boolean builtin call `name(operand, ..)`; the cursor is on the name.
    fn call(&mut self) -> Option<Expr> {
        let name = self.ident("a function name")?;
        if let Some(f) = shortest_fn(&name.name) {
            self.refuse(name.span, &format!("{f}()"), "write `p = ANY SHORTEST (a)-[:T]->+(b)`");
            return None;
        }
        if let Some(op) = agg_op(&name.name) {
            self.error(name.span, format!("aggregate `{}()` is allowed only in RETURN", op.name()));
            return None;
        }
        self.bump();
        let mut args = Vec::new();
        if !self.at(&TokenKind::RParen) {
            loop {
                args.push(self.operand()?);
                if !self.at(&TokenKind::Comma) {
                    break;
                }
                self.bump();
            }
        }
        self.expect(TokenKind::RParen, "`,` or `)` in the call")?;
        let compared = matches!(
            self.kind(),
            TokenKind::Eq
                | TokenKind::Ne
                | TokenKind::Lt
                | TokenKind::Le
                | TokenKind::Gt
                | TokenKind::Ge
        ) || ["STARTS", "ENDS", "CONTAINS", "IN"].into_iter().any(|w| self.kw(0, w));
        if compared {
            // `toUpper(a.x) = 'X'`: a call in WHERE is a condition, never a value.
            self.error(
                name.span,
                format!(
                    "function `{}()` {SUBSET} as a value: a condition may call a boolean \
                     builtin (`in_scope(a, 'web')`), and a value is {VALUE}",
                    name.name
                ),
            );
            return None;
        }
        Some(Expr::Call { span: self.span_from(name.span), name, args })
    }

    /// The operator and right side after `left`.
    fn comparison(&mut self, left: Operand) -> Option<Expr> {
        let start = left.span();
        let op = match self.kind() {
            TokenKind::Eq => Some(CmpOp::Eq),
            TokenKind::Ne => Some(CmpOp::Ne),
            TokenKind::Lt => Some(CmpOp::Lt),
            TokenKind::Le => Some(CmpOp::Le),
            TokenKind::Gt => Some(CmpOp::Gt),
            TokenKind::Ge => Some(CmpOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.bump();
            let right = self.operand()?;
            return Some(Expr::Cmp { op, left, right, span: self.span_from(start) });
        }
        let test = if self.kw(0, "STARTS") || self.kw(0, "ENDS") {
            let starts = self.kw(0, "STARTS");
            self.bump();
            if !self.kw(0, "WITH") {
                return self.unexpected("WITH");
            }
            Some(if starts { StrOp::StartsWith } else { StrOp::EndsWith })
        } else if self.kw(0, "CONTAINS") {
            Some(StrOp::Contains)
        } else {
            None
        };
        if let Some(op) = test {
            self.bump();
            let right = self.operand()?;
            return Some(Expr::StrTest { op, left, right, span: self.span_from(start) });
        }
        if self.kw(0, "IN") {
            self.bump();
            self.expect(TokenKind::LBracket, "`[` to open the IN list")?;
            let mut list = vec![self.lit()?];
            while self.at(&TokenKind::Comma) {
                self.bump();
                list.push(self.lit()?);
            }
            self.expect(TokenKind::RBracket, "`,` or `]` in the IN list")?;
            return Some(Expr::In { left, list, span: self.span_from(start) });
        }
        // `a<-1` lexes `<-` as an arrow: hint the spacing, read it as `a < -1`.
        if let (TokenKind::LeftArrow, TokenKind::Int(v)) = (self.kind(), &self.tok(1).kind) {
            let v = *v;
            let arrow = self.bump();
            let span = minus_through(arrow, self.bump());
            let shown = match &left {
                Operand::Prop { var, key, .. } => format!("{}.{}", var.name, key.name),
                Operand::Var(v) => v.name.clone(),
                _ => "x".to_string(),
            };
            self.error(arrow, format!("`<-` reads as an arrow here: write `{shown} < -{v}`"));
            let right = Operand::Lit(Lit::Int { value: v.checked_neg().unwrap_or(v), span });
            return Some(Expr::Cmp { op: CmpOp::Lt, left, right, span: self.span_from(start) });
        }
        self.unexpected(
            "a comparison (`=`, `<>`, `<`, `<=`, `>`, `>=`, STARTS WITH, ENDS WITH, CONTAINS or IN)",
        )
    }

    // ---- values ------------------------------------------------------------

    fn operand(&mut self) -> Option<Operand> {
        let Some(word) = self.word(0).map(str::to_string) else {
            return match self.kind() {
                TokenKind::Str(_) | TokenKind::Int(_) | TokenKind::Minus => {
                    Some(Operand::Lit(self.lit()?))
                }
                _ => self.unexpected(VALUE),
            };
        };
        if word.eq_ignore_ascii_case("TRUE") || word.eq_ignore_ascii_case("FALSE") {
            return Some(Operand::Lit(self.lit()?));
        }
        if word.eq_ignore_ascii_case("CASE") {
            let span = self.bump();
            self.refuse(span, "CASE", "");
            return None;
        }
        if reserved(&word) {
            return self.unexpected(VALUE);
        }
        match self.tok(1).kind {
            TokenKind::LParen if word.eq_ignore_ascii_case("type") => {
                let start = self.bump();
                self.bump();
                let var = self.var("a relationship variable in type()")?;
                self.expect(TokenKind::RParen, "`)` to close type()")?;
                Some(Operand::Type { var, span: self.span_from(start) })
            }
            TokenKind::LParen => {
                let span = self.tok(0).span;
                match shortest_fn(&word) {
                    Some(f) => self.refuse(
                        span,
                        &format!("{f}()"),
                        "write `p = ANY SHORTEST (a)-[:T]->+(b)`",
                    ),
                    None => self
                        .error(span, format!("function `{word}()` {SUBSET}: a value is {VALUE}")),
                }
                None
            }
            TokenKind::Dot => {
                let var = self.ident("a variable")?;
                self.bump();
                let key = self.ident("a property key after `.`")?;
                Some(Operand::Prop { span: join(var.span, key.span), var, key })
            }
            _ => Some(Operand::Var(self.ident("a variable")?)),
        }
    }

    fn lit(&mut self) -> Option<Lit> {
        let span = self.tok(0).span;
        match self.kind().clone() {
            TokenKind::Str(value) => {
                self.bump();
                Some(Lit::Str { value, span })
            }
            TokenKind::Int(value) => {
                self.bump();
                Some(Lit::Int { value, span })
            }
            TokenKind::Minus if matches!(self.tok(1).kind, TokenKind::Int(_)) => {
                self.bump();
                let TokenKind::Int(v) = *self.kind() else { return self.unexpected("an integer") };
                let span = join(span, self.bump());
                match v.checked_neg() {
                    Some(value) => Some(Lit::Int { value, span }),
                    None => {
                        self.error(span, "integer out of range");
                        None
                    }
                }
            }
            TokenKind::Ident(w)
                if w.eq_ignore_ascii_case("TRUE") || w.eq_ignore_ascii_case("FALSE") =>
            {
                self.bump();
                Some(Lit::Bool { value: w.eq_ignore_ascii_case("TRUE"), span })
            }
            _ => self.unexpected("a literal (a string, an integer, TRUE or FALSE)"),
        }
    }
}
