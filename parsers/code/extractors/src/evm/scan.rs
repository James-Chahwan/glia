//! The code view and the text helpers every EVM client reader scans with
//! (DT.10c).
//!
//! [`code_view`] is the source with its comments, regex literals and the
//! CONTENTS of its string and template literals blanked to spaces: byte
//! offsets are preserved (newlines are kept, so a line in the view is the
//! same line in the source) and the quotes stay, so a reader finds every
//! needle in the view and reads every literal from the source at the same
//! offset ([`string_literal`]). A `${..}` expression inside a template is
//! blanked with the template. A `/../` is a regex only after an operator,
//! `(`, `,`, `=` or a keyword such as `return` (the usual lexer rule); `<` /
//! `>` are left out so a JSX closing tag never opens one.
//!
//! The view's brackets are balanced wherever the source's code brackets are,
//! which is what [`matching`], [`split_args`] and [`enclosing_block`] count.

use super::TS_FAMILY;

/// Deepest `${ .. `..` .. }` template nesting the lexer follows; past it the
/// rest of the template is read as template text.
const MAX_NEST: usize = 16;

/// Bytes after which a `/` opens a regex literal (module doc).
const REGEX_AFTER: &[u8] = b"(,=:[!&|?{};+-*%~^";

/// Keywords after which a `/` opens a regex literal.
const REGEX_KEYWORDS: &[&[u8]] = &[
    b"return", b"typeof", b"case", b"yield", b"await", b"throw", b"void", b"delete", b"in", b"of",
    b"else", b"do",
];

/// The code view of `source` (module doc). A TS-family tag gets the
/// JavaScript lexer; a language this module has no lexer for gets its source
/// back unchanged.
pub(crate) fn code_view(source: &str, lang: &str) -> String {
    if !TS_FAMILY.contains(&lang) {
        return source.to_string();
    }
    let src = source.as_bytes();
    let mut out = src.to_vec();
    let mut prev: Option<usize> = None;
    let mut i = 0;
    while i < src.len() {
        let b = src[i];
        let next = src.get(i + 1).copied();
        if b == b'/' && next == Some(b'/') {
            let end = line_end(src, i);
            blank(&mut out, i, end);
            i = end;
        } else if b == b'/' && next == Some(b'*') {
            let end = comment_end(src, i);
            blank(&mut out, i, end);
            i = end;
        } else if b == b'\'' || b == b'"' {
            let (end, closed) = string_end(src, i);
            blank(&mut out, i + 1, if closed { end - 1 } else { end });
            prev = Some(i);
            i = end;
        } else if b == b'`' {
            let end = template_end(src, i, 0);
            let closed = end > i + 1 && src.get(end - 1) == Some(&b'`');
            blank(&mut out, i + 1, if closed { end - 1 } else { end });
            prev = Some(i);
            i = end;
        } else if b == b'/' && regex_may_start(src, prev) {
            if let Some(end) = regex_end(src, i) {
                blank(&mut out, i + 1, end);
                i = end;
            } else {
                i += 1;
            }
            prev = Some(i.saturating_sub(1));
        } else {
            if !b.is_ascii_whitespace() {
                prev = Some(i);
            }
            i += 1;
        }
    }
    // Every blanked range starts and ends at an ASCII delimiter, so `out` is
    // UTF-8; the fallback (never taken) keeps every offset all the same.
    String::from_utf8(out).unwrap_or_else(|e| {
        e.into_bytes()
            .into_iter()
            .map(|c| if c.is_ascii() { c as char } else { ' ' })
            .collect()
    })
}

/// Spaces over `out[from..to]`, newlines kept.
fn blank(out: &mut [u8], from: usize, to: usize) {
    let to = to.min(out.len());
    for c in out.iter_mut().take(to).skip(from) {
        if *c != b'\n' {
            *c = b' ';
        }
    }
}

/// The offset of the `\n` ending the line at `from` (or the end).
fn line_end(src: &[u8], from: usize) -> usize {
    src[from..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(src.len(), |p| from + p)
}

/// The offset just past the `*/` closing the block comment at `from`.
fn comment_end(src: &[u8], from: usize) -> usize {
    src.get(from + 2..)
        .and_then(|rest| rest.windows(2).position(|w| w == b"*/"))
        .map_or(src.len(), |p| from + 2 + p + 2)
}

/// The offset just past the string literal opening at `open`, and whether a
/// closing quote ended it (an unterminated one stops at its line's end).
fn string_end(src: &[u8], open: usize) -> (usize, bool) {
    let q = src[open];
    let mut j = open + 1;
    while j < src.len() {
        match src[j] {
            b'\\' => j += 2,
            b'\n' => return (j, false),
            c if c == q => return (j + 1, true),
            _ => j += 1,
        }
    }
    (src.len(), false)
}

/// The offset just past the template literal opening at `open`, following
/// `${..}` expressions (and the strings and templates inside them).
fn template_end(src: &[u8], open: usize, nest: usize) -> usize {
    let mut j = open + 1;
    while j < src.len() {
        match src[j] {
            b'\\' => j += 2,
            b'`' => return j + 1,
            b'$' if src.get(j + 1) == Some(&b'{') && nest < MAX_NEST => {
                j = expr_end(src, j + 2, nest + 1);
            }
            _ => j += 1,
        }
    }
    src.len()
}

/// The offset just past the `}` closing a template expression that starts at
/// `start`.
fn expr_end(src: &[u8], start: usize, nest: usize) -> usize {
    let mut depth = 0usize;
    let mut j = start;
    while j < src.len() {
        match src[j] {
            b'\'' | b'"' => j = string_end(src, j).0,
            b'`' => j = template_end(src, j, nest),
            b'/' if src.get(j + 1) == Some(&b'/') => j = line_end(src, j),
            b'/' if src.get(j + 1) == Some(&b'*') => j = comment_end(src, j),
            b'{' => {
                depth += 1;
                j += 1;
            }
            b'}' => {
                if depth == 0 {
                    return j + 1;
                }
                depth -= 1;
                j += 1;
            }
            _ => j += 1,
        }
    }
    src.len()
}

/// True when a `/` after the code byte at `prev` opens a regex literal.
fn regex_may_start(src: &[u8], prev: Option<usize>) -> bool {
    let Some(p) = prev else {
        return true;
    };
    let c = src[p];
    if REGEX_AFTER.contains(&c) {
        return true;
    }
    if is_ident_byte(c) {
        let start = (0..=p)
            .rev()
            .take_while(|&k| is_ident_byte(src[k]))
            .last()
            .unwrap_or(p);
        return REGEX_KEYWORDS.contains(&&src[start..=p]);
    }
    false
}

/// The offset just past the regex literal (and its flags) opening at `open`;
/// `None` when no closing `/` is on its line, so the `/` was a division.
fn regex_end(src: &[u8], open: usize) -> Option<usize> {
    let mut class = false;
    let mut j = open + 1;
    while j < src.len() {
        match src[j] {
            b'\\' => j += 2,
            b'\n' => return None,
            b'[' => {
                class = true;
                j += 1;
            }
            b']' => {
                class = false;
                j += 1;
            }
            b'/' if !class => {
                j += 1;
                while j < src.len() && src[j].is_ascii_alphabetic() {
                    j += 1;
                }
                return Some(j);
            }
            _ => j += 1,
        }
    }
    None
}

// ---- brackets -----------------------------------------------------------------

/// The offset of the bracket closing the `(` / `[` / `{` at `open`, counting
/// every bracket kind in the view; `None` when `open` is no opener or the
/// bracket never closes.
pub(crate) fn matching(view: &str, open: usize) -> Option<usize> {
    let b = view.as_bytes();
    if !matches!(b.get(open), Some(b'(' | b'[' | b'{')) {
        return None;
    }
    let mut depth = 0usize;
    for (k, &c) in b.iter().enumerate().skip(open) {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// The argument ranges between the brackets at `open` and `close`, split at
/// depth-0 commas and trimmed of whitespace; an empty piece (`()`, a trailing
/// comma) is no argument.
pub(crate) fn split_args(view: &str, open: usize, close: usize) -> Vec<(usize, usize)> {
    let b = view.as_bytes();
    let mut out = Vec::new();
    if close <= open || close > b.len() {
        return out;
    }
    let mut depth = 0usize;
    let mut start = open + 1;
    let push = |from: usize, to: usize, out: &mut Vec<(usize, usize)>| {
        let (s, e) = trim(b, from, to);
        if s < e {
            out.push((s, e));
        }
    };
    for k in open + 1..close {
        match b[k] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                push(start, k, &mut out);
                start = k + 1;
            }
            _ => {}
        }
    }
    push(start, close, &mut out);
    out
}

/// `[from, to)` without its leading and trailing whitespace.
pub(crate) fn trim(b: &[u8], mut from: usize, mut to: usize) -> (usize, usize) {
    while from < to && b[from].is_ascii_whitespace() {
        from += 1;
    }
    while to > from && b[to - 1].is_ascii_whitespace() {
        to -= 1;
    }
    (from, to)
}

/// The innermost balanced `{..}` of the view holding `at`, as the offsets of
/// its two braces; the whole file `(0, len)` at top level.
pub(crate) fn enclosing_block(view: &str, at: usize) -> (usize, usize) {
    let b = view.as_bytes();
    let mut depth = 0usize;
    let mut k = at.min(b.len());
    while k > 0 {
        k -= 1;
        match b[k] {
            b'}' => depth += 1,
            b'{' if depth > 0 => depth -= 1,
            b'{' => {
                if let Some(close) = brace_close(b, k)
                    && close >= at
                {
                    return (k, close);
                }
            }
            _ => {}
        }
    }
    (0, b.len())
}

/// The offset of the `}` closing the `{` at `open`, counting braces only.
fn brace_close(b: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (k, &c) in b.iter().enumerate().skip(open) {
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

// ---- literals -----------------------------------------------------------------

/// The text of the string literal whose opening quote (`'`, `"` or a
/// backtick) is at `at` in `source`, unescaped, and the offset just past its
/// closing quote. `None` for an unterminated literal and for a template that
/// holds a `${..}` expression.
pub(crate) fn string_literal(source: &str, at: usize) -> Option<(String, usize)> {
    let b = source.as_bytes();
    let q = *b.get(at)?;
    if !matches!(q, b'\'' | b'"' | b'`') {
        return None;
    }
    let mut text = String::new();
    let mut chars = source.get(at + 1..)?.char_indices();
    while let Some((k, c)) = chars.next() {
        match c {
            '\\' => {
                let (_, e) = chars.next()?;
                text.push(match e {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    other => other,
                });
            }
            '\n' if q != b'`' => return None,
            '$' if q == b'`' && source[at + 1 + k..].starts_with("${") => return None,
            c if c as u32 == u32::from(q) => return Some((text, at + 1 + k + 1)),
            c => text.push(c),
        }
    }
    None
}

// ---- identifiers --------------------------------------------------------------

/// A byte that can continue a JavaScript identifier (ASCII view).
pub(crate) fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'$'
}

/// A byte that can start a JavaScript identifier (ASCII view).
fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$'
}

/// The end of the identifier starting at `at`; `None` when none starts there.
pub(crate) fn ident_end(b: &[u8], at: usize) -> Option<usize> {
    if !b.get(at).copied().is_some_and(is_ident_start) {
        return None;
    }
    Some(
        (at..b.len())
            .find(|&k| !is_ident_byte(b[k]))
            .unwrap_or(b.len()),
    )
}

/// The start of the identifier ending just before `end`; `None` when none
/// ends there.
pub(crate) fn ident_start_before(b: &[u8], end: usize) -> Option<usize> {
    let mut s = end;
    while s > 0 && is_ident_byte(b[s - 1]) {
        s -= 1;
    }
    (s < end && is_ident_start(b[s])).then_some(s)
}

/// True when `text` is one identifier.
pub(crate) fn is_ident(text: &str) -> bool {
    ident_end(text.as_bytes(), 0) == Some(text.len())
}

/// The first offset at or after `at` that is no whitespace.
pub(crate) fn skip_ws(b: &[u8], mut at: usize) -> usize {
    while at < b.len() && b[at].is_ascii_whitespace() {
        at += 1;
    }
    at
}

/// The offset of the last byte before `at` that is no whitespace.
pub(crate) fn prev_non_ws(b: &[u8], at: usize) -> Option<usize> {
    (0..at.min(b.len()))
        .rev()
        .find(|&k| !b[k].is_ascii_whitespace())
}

/// Every offset where `word` stands in the view as a whole identifier.
pub(crate) fn word_hits(view: &str, word: &str) -> Vec<usize> {
    let b = view.as_bytes();
    view.match_indices(word)
        .map(|(at, _)| at)
        .filter(|&at| {
            (at == 0 || !is_ident_byte(b[at - 1]))
                && b.get(at + word.len()).is_none_or(|&c| !is_ident_byte(c))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(src: &str) -> String {
        code_view(src, "typescript")
    }

    #[test]
    fn code_view_blanks_comments_and_literal_contents_keeping_offsets() {
        let src = "const a = 'x.f(1)'; // b.g(2)\n/* c.h(3)\n */ let t = `d ${e.i(4)} f`;";
        let v = view(src);
        assert_eq!(v.len(), src.len());
        assert_eq!(v.matches('\n').count(), src.matches('\n').count());
        assert!(!v.contains("f(1)") && !v.contains("g(2)") && !v.contains("h(3)"));
        assert!(
            !v.contains("i(4)"),
            "a template expression is blanked: {v:?}"
        );
        assert!(v.contains("const a = '      ';"));
        assert!(v.contains("let t = `"));
    }

    #[test]
    fn code_view_reads_regex_only_where_one_can_start() {
        let src = "const r = /a{b/g; const q = x / y / z; f(/}/);";
        let v = view(src);
        assert!(!v.contains("a{b") && !v.contains("/}/"), "{v:?}");
        assert!(v.contains("x / y / z"), "a division stays: {v:?}");
        let jsx = "<p>{a}</p><b>{c}</b>";
        assert_eq!(view(jsx), jsx, "a closing tag never opens a regex");
    }

    #[test]
    fn code_view_returns_other_languages_unchanged() {
        assert_eq!(code_view("x // y", "python"), "x // y");
    }

    #[test]
    fn brackets_args_and_blocks() {
        let src = "f(a, g(b, c), [d, e], { h: 1 },)";
        let v = view(src);
        let close = matching(&v, 1).expect("closes");
        assert_eq!(close, src.len() - 1);
        let args: Vec<&str> = split_args(&v, 1, close)
            .iter()
            .map(|&(s, e)| &src[s..e])
            .collect();
        assert_eq!(args, ["a", "g(b, c)", "[d, e]", "{ h: 1 }"]);
        assert!(split_args("f()", 1, 2).is_empty());

        let src = "a; function f() { if (x) { y; } z; } w;";
        let at_y = src.find('y').expect("y");
        let at_z = src.find('z').expect("z");
        let (open, close) = enclosing_block(src, at_y);
        assert_eq!(&src[open..=close], "{ y; }");
        let (open, close) = enclosing_block(src, at_z);
        assert_eq!(&src[open..=close], "{ if (x) { y; } z; }");
        assert_eq!(
            enclosing_block(src, src.find('w').expect("w")),
            (0, src.len())
        );
    }

    #[test]
    fn string_literals_unescape_and_refuse_interpolation() {
        let src = r#"x("it\'s", `plain`, `a ${b}`)"#;
        let at = src.find('"').expect("quote");
        let (text, end) = string_literal(src, at).expect("reads");
        assert_eq!(text, "it's");
        assert_eq!(&src[end..end + 1], ",");
        let at = src.find("`plain").expect("template");
        assert_eq!(
            string_literal(src, at).map(|t| t.0).as_deref(),
            Some("plain")
        );
        let at = src.find("`a").expect("template");
        assert_eq!(string_literal(src, at), None);
        assert_eq!(string_literal("'open", 0), None);
    }

    #[test]
    fn identifier_helpers() {
        let b = b"  foo.bar(";
        assert_eq!(ident_end(b, 2), Some(5));
        assert_eq!(ident_end(b, 1), None);
        assert_eq!(ident_start_before(b, 5), Some(2));
        assert!(is_ident("ESCROW_ABI") && !is_ident("a.b") && !is_ident("1x"));
        assert_eq!(
            word_hits("escrow escrowS x.escrow escrow", "escrow"),
            vec![0, 17, 24]
        );
        assert_eq!(prev_non_ws(b"a  b", 3), Some(0));
        assert_eq!(skip_ws(b"a  b", 1), 3);
    }
}
