//! The one tokenizer for the Datalog surface and the GQL read subset (R1 §5: one
//! shared lexer for both surfaces, hand-rolled for span control). [`lex`] turns a
//! rule or query text into [`Token`]s ending with [`TokenKind::Eof`]; the Datalog
//! parser (`parser`) and the GQL parser (`gql`) both read that stream.
//!
//! **Tokens.** Identifiers are `[A-Za-z_][A-Za-z0-9_]*`. Keywords are identifiers:
//! the GQL parser compares `MATCH` / `WHERE` / `WITH` ... case-insensitively, the
//! Datalog parser reads `not` / `count` / `min` / `max` / `sum` by position, and a
//! lone `_` is `Ident("_")` (the parser makes it the wildcard). Integers are decimal
//! digits only, with no floats, so `1..3` lexes `Int DotDot Int`. Strings take
//! `'...'` or `"..."` with the escapes `\\` `\"` `\'` `\n` `\t`; a raw tab or CR
//! inside a string is kept as written. Comments run `//` to end of line or
//! `/* ... */` (not nested). Punctuation: `( ) [ ] { } , . .. : :- ; | * + - ? -> <-
//! ! = != <> < <= > >= $`, where `!=` and `<>` are both `Ne` and `$` (a GQL
//! parameter) is lexed so the GQL parser can refuse it by name.
//!
//! **Minus and the greedy pairs.** A leading minus is its own `Minus` token; the
//! parser folds `-` `Int` into a negative literal. The one exception is the integer
//! 9223372036854775808, whose negation is `i64::MIN`: it lexes only right after a
//! `Minus`, and the two become one `Int(i64::MIN)` token spanning both, so the
//! canonical rendering `n < -9223372036854775808` reads back. Two-character
//! operators lex greedily, so `<-` is always `LeftArrow` and `:-` always
//! `ColonDash`: a Datalog comparison with a negative constant needs a space
//! (`a < -1`, never `a<-1`), and so does a GQL property (`{n: -1}`, never `{n:-1}`).
//!
//! **Spans.** Offsets are BYTES and columns are CHARS: the walk advances one whole
//! char at a time, so a span's `start..end` always falls on char boundaries and the
//! text is never sliced mid-char (or by column). `line` is 1-based and counts `\n`;
//! `col` is 1 + the chars since the line start. A CR is an ordinary char, so CRLF
//! input reports the same lines and columns as LF. A leading byte-order mark is
//! skipped without counting a column.
//!
//! **Errors.** Every lexical error is collected, up to 20, before [`lex`] returns
//! `RulesError::Parse`, so one bad character does not hide the next: `unexpected
//! character `@``, `unterminated string`, `unknown escape `\q` in string`,
//! `unterminated block comment` and `integer out of range`. A string error is
//! located at its opening quote and a comment error at its `/*`. A diagnostic here
//! names no predicate (no clause head is known yet), so it renders
//! `<source>:<line>:<col>: error: <message>`.

use std::fmt;

use crate::diag::{Diagnostic, RulesError, Span};
use crate::ir::{SourceId, Term};

/// The most diagnostics one lex (and one parse) reports.
pub(crate) const MAX_DIAGNOSTICS: usize = 20;

/// What a token is. `Ident`, `Str` and `Int` carry their value (a string with its
/// escapes applied); every other kind is a fixed spelling.
#[derive(Clone, Debug, PartialEq)]
pub enum TokenKind {
    Ident(String),
    Str(String),
    Int(i64),
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Dot,
    /// `..`
    DotDot,
    Colon,
    /// `:-`
    ColonDash,
    Semicolon,
    Pipe,
    Star,
    Plus,
    Minus,
    Question,
    /// `->`
    Arrow,
    /// `<-`
    LeftArrow,
    Bang,
    Eq,
    /// `!=` or `<>`
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `$`, a GQL parameter: lexed so the GQL parser can refuse it by name.
    Dollar,
    Eof,
}

/// The token's canonical spelling, for a parser's "found `...`" message: an
/// identifier or integer as written, a string double-quoted with the IR's escapes
/// (`Term`'s Display), `Ne` as `!=` and `Eof` as `end of input`. The exact source
/// text of a token is `text.get(span.start..span.end)`.
impl fmt::Display for TokenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let symbol = match self {
            TokenKind::Ident(s) => return f.write_str(s),
            TokenKind::Str(s) => return fmt::Display::fmt(&Term::str(s.as_str()), f),
            TokenKind::Int(v) => return write!(f, "{v}"),
            TokenKind::LParen => "(",
            TokenKind::RParen => ")",
            TokenKind::LBracket => "[",
            TokenKind::RBracket => "]",
            TokenKind::LBrace => "{",
            TokenKind::RBrace => "}",
            TokenKind::Comma => ",",
            TokenKind::Dot => ".",
            TokenKind::DotDot => "..",
            TokenKind::Colon => ":",
            TokenKind::ColonDash => ":-",
            TokenKind::Semicolon => ";",
            TokenKind::Pipe => "|",
            TokenKind::Star => "*",
            TokenKind::Plus => "+",
            TokenKind::Minus => "-",
            TokenKind::Question => "?",
            TokenKind::Arrow => "->",
            TokenKind::LeftArrow => "<-",
            TokenKind::Bang => "!",
            TokenKind::Eq => "=",
            TokenKind::Ne => "!=",
            TokenKind::Lt => "<",
            TokenKind::Le => "<=",
            TokenKind::Gt => ">",
            TokenKind::Ge => ">=",
            TokenKind::Dollar => "$",
            TokenKind::Eof => "end of input",
        };
        f.write_str(symbol)
    }
}

/// One token and where it sits in its source.
#[derive(Clone, Debug, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

/// Tokenize `text`, the source `source` named `source_name` (the name every
/// diagnostic renders). The tokens end with one `Eof` whose empty span sits at the
/// end of the text. Any lexical error returns `RulesError::Parse` with every
/// diagnostic found, at most 20, in source order.
pub fn lex(source: SourceId, source_name: &str, text: &str) -> Result<Vec<Token>, RulesError> {
    let mut lexer = Lexer {
        cur: Cursor::new(text),
        source,
        source_name,
        tokens: Vec::new(),
        diags: Vec::new(),
    };
    lexer.run();
    if lexer.diags.is_empty() { Ok(lexer.tokens) } else { Err(RulesError::Parse(lexer.diags)) }
}

/// A position: byte offset (always on a char boundary), 1-based line and char column.
#[derive(Clone, Copy)]
struct Mark {
    pos: usize,
    line: u32,
    col: u32,
}

/// A walk over `text` one whole char at a time.
struct Cursor<'a> {
    text: &'a str,
    at: Mark,
}

impl<'a> Cursor<'a> {
    fn new(text: &'a str) -> Self {
        // A byte-order mark is invisible in an editor, so it takes no column.
        let pos = if text.starts_with('\u{feff}') { '\u{feff}'.len_utf8() } else { 0 };
        Cursor { text, at: Mark { pos, line: 1, col: 1 } }
    }

    /// The unread text; `get` keeps a bad offset from panicking (it never is one).
    fn rest(&self) -> &'a str {
        self.text.get(self.at.pos..).unwrap_or("")
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn peek2(&self) -> Option<char> {
        let mut chars = self.rest().chars();
        chars.next();
        chars.next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at.pos += c.len_utf8();
        if c == '\n' {
            self.at.line = self.at.line.saturating_add(1);
            self.at.col = 1;
        } else {
            self.at.col = self.at.col.saturating_add(1);
        }
        Some(c)
    }

    /// Consume the next char when it is `want`.
    fn eat(&mut self, want: char) -> bool {
        if self.peek() == Some(want) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// The text from `from` to the cursor (ASCII-only callers: idents).
    fn since(&self, from: Mark) -> &'a str {
        self.text.get(from.pos..self.at.pos).unwrap_or("")
    }
}

struct Lexer<'a> {
    cur: Cursor<'a>,
    source: SourceId,
    source_name: &'a str,
    tokens: Vec<Token>,
    diags: Vec<Diagnostic>,
}

/// A byte offset as a span field; a text past 4 GiB pins at `u32::MAX`.
fn offset(pos: usize) -> u32 {
    u32::try_from(pos).unwrap_or(u32::MAX)
}

/// A char as a diagnostic shows it: control chars escaped, everything else as is.
fn shown(c: char) -> String {
    if c.is_control() { c.escape_default().to_string() } else { c.to_string() }
}

impl Lexer<'_> {
    fn span_from(&self, from: Mark) -> Span {
        Span {
            source: self.source,
            start: offset(from.pos),
            end: offset(self.cur.at.pos),
            line: from.line,
            col: from.col,
        }
    }

    fn error(&mut self, from: Mark, message: String) {
        if self.diags.len() < MAX_DIAGNOSTICS {
            let span = self.span_from(from);
            self.diags.push(Diagnostic::error(span, self.source_name, None, message));
        }
    }

    fn push(&mut self, from: Mark, kind: TokenKind) {
        let span = self.span_from(from);
        self.tokens.push(Token { kind, span });
    }

    fn run(&mut self) {
        while self.diags.len() < MAX_DIAGNOSTICS {
            self.skip_trivia();
            let from = self.cur.at;
            let Some(c) = self.cur.bump() else {
                self.push(from, TokenKind::Eof);
                return;
            };
            let kind = match c {
                '(' => TokenKind::LParen,
                ')' => TokenKind::RParen,
                '[' => TokenKind::LBracket,
                ']' => TokenKind::RBracket,
                '{' => TokenKind::LBrace,
                '}' => TokenKind::RBrace,
                ',' => TokenKind::Comma,
                ';' => TokenKind::Semicolon,
                '|' => TokenKind::Pipe,
                '*' => TokenKind::Star,
                '+' => TokenKind::Plus,
                '?' => TokenKind::Question,
                '=' => TokenKind::Eq,
                '$' => TokenKind::Dollar,
                '.' if self.cur.eat('.') => TokenKind::DotDot,
                '.' => TokenKind::Dot,
                ':' if self.cur.eat('-') => TokenKind::ColonDash,
                ':' => TokenKind::Colon,
                '-' if self.cur.eat('>') => TokenKind::Arrow,
                '-' => TokenKind::Minus,
                '!' if self.cur.eat('=') => TokenKind::Ne,
                '!' => TokenKind::Bang,
                '<' if self.cur.eat('-') => TokenKind::LeftArrow,
                '<' if self.cur.eat('=') => TokenKind::Le,
                '<' if self.cur.eat('>') => TokenKind::Ne,
                '<' => TokenKind::Lt,
                '>' if self.cur.eat('=') => TokenKind::Ge,
                '>' => TokenKind::Gt,
                '"' | '\'' => match self.string(c, from) {
                    Some(s) => TokenKind::Str(s),
                    None => continue,
                },
                '0'..='9' => {
                    self.integer(c, from);
                    continue;
                }
                c if c == '_' || c.is_ascii_alphabetic() => {
                    while self.cur.peek().is_some_and(|c| c == '_' || c.is_ascii_alphanumeric()) {
                        self.cur.bump();
                    }
                    TokenKind::Ident(self.cur.since(from).to_string())
                }
                other => {
                    self.error(from, format!("unexpected character `{}`", shown(other)));
                    continue;
                }
            };
            self.push(from, kind);
        }
    }

    /// Skip whitespace and comments; an unterminated block comment is an error at
    /// its `/*` and leaves the cursor at the end of the text.
    fn skip_trivia(&mut self) {
        loop {
            match (self.cur.peek(), self.cur.peek2()) {
                (Some(c), _) if c.is_whitespace() => {
                    self.cur.bump();
                }
                (Some('/'), Some('/')) => {
                    while self.cur.peek().is_some_and(|c| c != '\n') {
                        self.cur.bump();
                    }
                }
                (Some('/'), Some('*')) => {
                    let from = self.cur.at;
                    self.cur.bump();
                    self.cur.bump();
                    loop {
                        match self.cur.bump() {
                            Some('*') if self.cur.eat('/') => break,
                            Some(_) => {}
                            None => {
                                self.error(from, "unterminated block comment".to_string());
                                return;
                            }
                        }
                    }
                }
                _ => return,
            }
        }
    }

    /// The rest of a string opened by `quote` at `from`; `None` after recording its
    /// error (a raw newline or the end of the text before the closing quote, or an
    /// unknown escape), each located at the opening quote.
    fn string(&mut self, quote: char, from: Mark) -> Option<String> {
        let mut value = String::new();
        let mut bad_escape = None;
        loop {
            match self.cur.peek() {
                None | Some('\n') => {
                    // The newline stays unread, so lexing resumes on the next line.
                    self.error(from, "unterminated string".to_string());
                    return None;
                }
                Some(c) => {
                    self.cur.bump();
                    if c == quote {
                        break;
                    }
                    if c != '\\' {
                        value.push(c);
                        continue;
                    }
                    match self.cur.peek() {
                        // Reported as an unterminated string on the next turn.
                        None | Some('\n') => {}
                        Some(e) => {
                            self.cur.bump();
                            match e {
                                '\\' | '"' | '\'' => value.push(e),
                                'n' => value.push('\n'),
                                't' => value.push('\t'),
                                other => {
                                    bad_escape.get_or_insert(other);
                                }
                            }
                        }
                    }
                }
            }
        }
        match bad_escape {
            Some(e) => {
                let message = format!(
                    "unknown escape `\\{}` in string (escapes: \\\\ \\\" \\' \\n \\t)",
                    shown(e)
                );
                self.error(from, message);
                None
            }
            None => Some(value),
        }
    }

    /// The integer whose first digit `first_digit`, at `from`, was just read.
    /// 9223372036854775808 right after a `Minus` folds with it into `Int(i64::MIN)`.
    fn integer(&mut self, first_digit: char, from: Mark) {
        let mut first = from;
        let mut value = u64::from(first_digit) - u64::from('0');
        let mut in_range = true;
        while let Some(d) = self.cur.peek().filter(char::is_ascii_digit) {
            self.cur.bump();
            let digit = u64::from(d) - u64::from('0');
            match value.checked_mul(10).and_then(|v| v.checked_add(digit)) {
                Some(v) => value = v,
                None => in_range = false,
            }
        }
        let kind = match i64::try_from(value) {
            Ok(v) if in_range => TokenKind::Int(v),
            _ if in_range && value == i64::MIN.unsigned_abs() && self.after_minus() => {
                if let Some(minus) = self.tokens.pop() {
                    first = Mark {
                        pos: minus.span.start as usize,
                        line: minus.span.line,
                        col: minus.span.col,
                    };
                }
                TokenKind::Int(i64::MIN)
            }
            _ => {
                self.error(from, "integer out of range".to_string());
                return;
            }
        };
        self.push(first, kind);
    }

    fn after_minus(&self) -> bool {
        self.tokens.last().is_some_and(|t| t.kind == TokenKind::Minus)
    }
}

#[cfg(test)]
mod tests {
    use super::TokenKind::*;
    use super::*;

    fn tokens(text: &str) -> Vec<Token> {
        match lex(0, "rules", text) {
            Ok(t) => t,
            Err(e) => panic!("lex failed on {text:?}:\n{e}"),
        }
    }

    fn kinds(text: &str) -> Vec<TokenKind> {
        tokens(text).into_iter().map(|t| t.kind).collect()
    }

    fn errors(text: &str) -> Vec<String> {
        match lex(0, "rules", text) {
            Ok(t) => panic!("lexed {text:?} without error: {t:?}"),
            Err(e) => e.diagnostics().iter().map(|d| d.to_string()).collect(),
        }
    }

    fn id(s: &str) -> TokenKind {
        Ident(s.to_string())
    }

    fn st(s: &str) -> TokenKind {
        Str(s.to_string())
    }

    #[test]
    fn datalog_rule_tokens() {
        assert_eq!(
            kinds(r#"p(x) :- q(x, "A"), !r(x), x != 3."#),
            vec![
                id("p"),
                LParen,
                id("x"),
                RParen,
                ColonDash,
                id("q"),
                LParen,
                id("x"),
                Comma,
                st("A"),
                RParen,
                Comma,
                Bang,
                id("r"),
                LParen,
                id("x"),
                RParen,
                Comma,
                id("x"),
                Ne,
                Int(3),
                Dot,
                Eof,
            ]
        );
        // `_` is an identifier the parser reads as the wildcard; idents take digits
        // and underscores after the first char; a digit run stops at a letter.
        assert_eq!(
            kinds("_ _x1 not count(*) 3abc x1.y"),
            vec![
                id("_"),
                id("_x1"),
                id("not"),
                id("count"),
                LParen,
                Star,
                RParen,
                Int(3),
                id("abc"),
                id("x1"),
                Dot,
                id("y"),
                Eof,
            ]
        );
    }

    #[test]
    fn gql_pattern_tokens() {
        assert_eq!(
            kinds(
                "MATCH (a:ROUTE)-[:CALLS|USES*1..3]->(b) WHERE a.qname STARTS WITH 'x' RETURN a, count(b)"
            ),
            vec![
                id("MATCH"),
                LParen,
                id("a"),
                Colon,
                id("ROUTE"),
                RParen,
                Minus,
                LBracket,
                Colon,
                id("CALLS"),
                Pipe,
                id("USES"),
                Star,
                Int(1),
                DotDot,
                Int(3),
                RBracket,
                Arrow,
                LParen,
                id("b"),
                RParen,
                id("WHERE"),
                id("a"),
                Dot,
                id("qname"),
                id("STARTS"),
                id("WITH"),
                st("x"),
                id("RETURN"),
                id("a"),
                Comma,
                id("count"),
                LParen,
                id("b"),
                RParen,
                Eof,
            ]
        );
        assert_eq!(
            kinds("(a)<-[:T]-(b)"),
            vec![
                LParen,
                id("a"),
                RParen,
                LeftArrow,
                LBracket,
                Colon,
                id("T"),
                RBracket,
                Minus,
                LParen,
                id("b"),
                RParen,
                Eof,
            ]
        );
        assert_eq!(kinds("->{1,3}"), vec![Arrow, LBrace, Int(1), Comma, Int(3), RBrace, Eof]);
        // The abbreviated arrows and the open quantifier, as DB.7a's grammar reads them.
        assert_eq!(kinds("-->"), vec![Minus, Arrow, Eof]);
        assert_eq!(kinds("<--"), vec![LeftArrow, Minus, Eof]);
        assert_eq!(kinds("--"), vec![Minus, Minus, Eof]);
        assert_eq!(
            kinds("]->+{2,}"),
            vec![RBracket, Arrow, Plus, LBrace, Int(2), Comma, RBrace, Eof]
        );
        assert_eq!(
            kinds("p = ANY SHORTEST (a {name: \"x\"});"),
            vec![
                id("p"),
                Eq,
                id("ANY"),
                id("SHORTEST"),
                LParen,
                id("a"),
                LBrace,
                id("name"),
                Colon,
                st("x"),
                RBrace,
                RParen,
                Semicolon,
                Eof,
            ]
        );
    }

    #[test]
    fn spans_count_chars() {
        let toks = tokens("\n\"é\" x");
        let x = &toks[1];
        assert_eq!(x.kind, id("x"));
        assert_eq!((x.span.line, x.span.col, x.span.start, x.span.end), (2, 5, 6, 7));
        // The string spans its quotes: bytes 1..5, the é two bytes wide.
        assert_eq!((toks[0].span.start, toks[0].span.end, toks[0].span.col), (1, 5, 1));
        // The end-of-input token is empty and sits at the end.
        assert_eq!((toks[2].kind.clone(), toks[2].span.start, toks[2].span.end), (Eof, 7, 7));

        // CRLF reports the same lines and columns as LF; only the bytes move.
        let lf = tokens("p(x).\nq(y) :-\n  r(y).\n");
        let crlf = tokens("p(x).\r\nq(y) :-\r\n  r(y).\r\n");
        let at = |t: &[Token]| t.iter().map(|t| (t.span.line, t.span.col)).collect::<Vec<_>>();
        assert_eq!(at(&lf), at(&crlf));
        let r = &crlf[10];
        assert_eq!((r.kind.clone(), r.span.line, r.span.col, r.span.start), (id("r"), 3, 3, 18));
        assert_eq!((lf[15].kind.clone(), lf[15].span.line, lf[15].span.col), (Eof, 4, 1));

        // Every span lands on char boundaries and slices back to its token's text.
        let text = "p(\"é\", 'ü') :- /* ö */ q(\"ßß\", x). // ü";
        let toks = tokens(text);
        assert_eq!(toks.len(), 15);
        for t in &toks {
            let slice = text.get(t.span.start as usize..t.span.end as usize);
            assert!(slice.is_some(), "span {:?} is not on a char boundary", t.span);
        }
        assert_eq!(text.get(toks[4].span.start as usize..toks[4].span.end as usize), Some("'ü'"));
        assert_eq!((toks[7].kind.clone(), toks[7].span.col), (id("q"), 24));
        let toks = tokens("ab(\"ßß\", cd)");
        let cd = &toks[4];
        assert_eq!((cd.kind.clone(), cd.span.col, cd.span.start), (id("cd"), 10, 11));

        // The span carries the source id; a leading byte-order mark takes no column.
        let toks = lex(7, "check", "\u{feff}ok.").unwrap();
        assert_eq!(toks[0].span, Span { source: 7, start: 3, end: 5, line: 1, col: 1 });
    }

    #[test]
    fn comments_and_escapes() {
        assert_eq!(kinds("// c\n/* b */ \"a\\\\\\\"b\""), vec![st("a\\\"b"), Eof]);
        // Every escape in both quote styles; a raw tab and CR are kept as written.
        assert_eq!(
            kinds(r#"'it\'s \"q\" \\ \n\t' "x'y" "#),
            vec![st("it's \"q\" \\ \n\t"), st("x'y"), Eof]
        );
        assert_eq!(kinds("\"a\tb\rc\""), vec![st("a\tb\rc"), Eof]);
        // A comment ends a line without eating the next token; a block comment may
        // span lines, and `*` inside it does not end it.
        let toks = tokens("a // x\nb /* 1\n * 2 **/ c");
        let at: Vec<_> = toks.iter().map(|t| (t.kind.clone(), t.span.line, t.span.col)).collect();
        assert_eq!(at, vec![(id("a"), 1, 1), (id("b"), 2, 1), (id("c"), 3, 10), (Eof, 3, 11)]);
        // The IR's canonical string rendering reads back to the same value.
        let value = "q\\\"t\n\tr\r'é";
        let rendered = Term::str(value).to_string();
        assert_eq!(kinds(&rendered), vec![st(value), Eof]);
    }

    #[test]
    fn lexical_errors_are_located() {
        let err = lex(0, "rules", "p(x).\nq(x,\"abc").unwrap_err();
        assert_eq!(err.diagnostics().len(), 1);
        assert_eq!(err.to_string(), "rules:2:5: error: unterminated string");
        let d = &err.diagnostics()[0];
        assert_eq!((d.span.start, d.span.end, d.predicate.clone()), (10, 14, None));

        assert_eq!(errors("99999999999999999999"), vec!["rules:1:1: error: integer out of range"]);
        assert_eq!(
            errors("p(@).\nq(#)."),
            vec![
                "rules:1:3: error: unexpected character `@`",
                "rules:2:3: error: unexpected character `#`",
            ]
        );
        // A raw newline ends the string; lexing resumes on the next line.
        assert_eq!(
            errors("\"ab\n@"),
            vec![
                "rules:1:1: error: unterminated string",
                "rules:2:1: error: unexpected character `@`"
            ]
        );
        // An unknown escape is located at the opening quote, once per string.
        assert_eq!(
            errors("x = 'a\\qb\\z'"),
            vec![
                "rules:1:5: error: unknown escape `\\q` in string (escapes: \\\\ \\\" \\' \\n \\t)"
            ]
        );
        assert_eq!(errors("p. /* x\ny"), vec!["rules:1:4: error: unterminated block comment"]);
        assert_eq!(errors("a / b"), vec!["rules:1:3: error: unexpected character `/`"]);
        // A non-ASCII char outside a string is one diagnostic spanning its bytes; a
        // control char is shown escaped.
        let err = lex(0, "rules", "é\u{7}").unwrap_err();
        let shown: Vec<_> =
            err.diagnostics().iter().map(|d| (d.to_string(), d.span.start, d.span.end)).collect();
        assert_eq!(
            shown,
            vec![
                ("rules:1:1: error: unexpected character `é`".to_string(), 0, 2),
                ("rules:1:2: error: unexpected character `\\u{7}`".to_string(), 2, 3),
            ]
        );
        // At most 20 diagnostics, in source order.
        let many = errors(&"@ ".repeat(30));
        assert_eq!(many.len(), MAX_DIAGNOSTICS);
        assert_eq!(many[19], "rules:1:39: error: unexpected character `@`");
    }

    #[test]
    fn ne_spellings() {
        assert_eq!(kinds("a != b <> c"), vec![id("a"), Ne, id("b"), Ne, id("c"), Eof]);
        assert_eq!(
            kinds("<= >= -> <- :- .. < > = ! : . ? $p"),
            vec![
                Le,
                Ge,
                Arrow,
                LeftArrow,
                ColonDash,
                DotDot,
                Lt,
                Gt,
                Eq,
                Bang,
                Colon,
                Dot,
                Question,
                Dollar,
                id("p"),
                Eof
            ]
        );
        let toks = tokens("x<>y");
        assert_eq!((toks[1].span.start, toks[1].span.end, toks[1].span.col), (1, 3, 2));
        // The greedy pairs: `a<-1` is an arrow, `a < -1` a comparison; `n:-1` is `:-`.
        assert_eq!(kinds("a<-1"), vec![id("a"), LeftArrow, Int(1), Eof]);
        assert_eq!(kinds("a < -1"), vec![id("a"), Lt, Minus, Int(1), Eof]);
        assert_eq!(kinds("{n:-1}"), vec![LBrace, id("n"), ColonDash, Int(1), RBrace, Eof]);
        assert_eq!(kinds("{n: -1}"), vec![LBrace, id("n"), Colon, Minus, Int(1), RBrace, Eof]);
    }

    #[test]
    fn integer_bounds() {
        assert_eq!(kinds("9223372036854775807 007"), vec![Int(i64::MAX), Int(7), Eof]);
        assert_eq!(errors("9223372036854775808"), vec!["rules:1:1: error: integer out of range"]);
        // Its negation is i64::MIN: after a minus the two fold into one token
        // spanning both, so the IR's rendering of i64::MIN reads back.
        let toks = tokens("n < -9223372036854775808.");
        assert_eq!(toks[2].kind, Int(i64::MIN));
        assert_eq!((toks[2].span.start, toks[2].span.end, toks[2].span.col), (4, 24, 5));
        assert_eq!(toks[3].kind, Dot);
        assert_eq!(kinds(&Term::int(i64::MIN).to_string()), vec![Int(i64::MIN), Eof]);
        assert_eq!(kinds("- 9223372036854775808"), vec![Int(i64::MIN), Eof]);
        assert_eq!(kinds("--9223372036854775808"), vec![Minus, Int(i64::MIN), Eof]);
        assert_eq!(kinds("-5"), vec![Minus, Int(5), Eof]);
        assert_eq!(errors("-9223372036854775809"), vec!["rules:1:2: error: integer out of range"]);
        assert_eq!(
            errors("1..99999999999999999999"),
            vec!["rules:1:4: error: integer out of range"]
        );
    }

    #[test]
    fn display_relexes() {
        // Every token kind's Display lexes back to that kind.
        let text = "id 'it\\'s' 42 ( ) [ ] { } , . .. : :- ; | * + - ? -> <- ! = != <> < <= > >= $";
        let original = kinds(text);
        let rendered: Vec<String> =
            original.iter().filter(|k| **k != Eof).map(ToString::to_string).collect();
        assert_eq!(rendered[1], "\"it's\"");
        assert_eq!(rendered[25], "!=");
        assert_eq!(kinds(&rendered.join(" ")), original);
        assert_eq!(Eof.to_string(), "end of input");
        assert_eq!(kinds(""), vec![Eof]);
        assert_eq!(tokens("p.\n")[2].span, Span { source: 0, start: 3, end: 3, line: 2, col: 1 });
    }
}
