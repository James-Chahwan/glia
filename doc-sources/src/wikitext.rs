//! MediaWiki **wikitext** → markdown (CE.4c), for `.mediawiki` / `.wiki` pages.
//!
//! The doc→code linker needs two things from a page: its `#` / `##` headings
//! (the engine's chunker cuts DOC_SECTIONs there) and its code spans (backticks
//! and fenced blocks, which make a mention Strong). Wikitext carries both
//! literally, so this converts it directly, with no HTML renderer and no
//! dependency:
//!   - `== H ==` → `## H` (as many `#` as `=`, at most six)
//!   - `<syntaxhighlight lang=x>` / `<source lang=x>` / `<pre>` → a fenced block,
//!     body verbatim (entities decoded); a run of lines opening with a space →
//!     a plain fenced block; `<syntaxhighlight inline>`, `<code>`, `<tt>` →
//!     `` `x` ``
//!   - `*` / `#` / `:` / `;` lists → `- ` / `1. ` / indentation; `{| … |}`
//!     tables → one line per row, cells joined by ` | `; `----` → `---`
//!   - `'''b'''` → `**b**`, `''i''` → `*i*`, `[[T|label]]` → `label`, `[[T]]` → `T`,
//!     `[https://u label]` → `[label](https://u)`
//!   - dropped: `{{templates}}` (nested; unbalanced → to the end of the page),
//!     `<!-- -->`, `<ref>`s, `[[File:]]` / `[[Category:]]`, `__TOC__`-style magic
//!     words, formatting tags (`<span>`, `<div>` …); `<nowiki>` content is kept
//!     as literal text.
//!
//! Template output is lost without a renderer: [`WikitextStats`] counts the
//! templates dropped and the constructs left open.
//!
//! Two passes, both linear: `Scan` consumes the multi-line constructs
//! (templates, comments, tags) and leaves each code span, `<nowiki>` run and
//! code block behind as a private-use placeholder, so no later rule can touch
//! its content; `Lines` then converts line by line. Every index is a
//! `char_indices` step or the position of an ASCII delimiter, so nothing is
//! sliced mid-char.

use std::collections::HashMap;

/// What one [`wikitext_to_markdown`] converted and dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WikitextStats {
    /// Headings written (`#` … `######`).
    pub headings: usize,
    /// Fenced blocks written: `<syntaxhighlight>` / `<source>` / `<pre>` and
    /// space-indented runs.
    pub code_blocks: usize,
    /// Inline code spans written: `<code>`, `<tt>`, `<syntaxhighlight inline>`.
    pub inline_code: usize,
    /// Wiki links and external links written (a dropped File / Category link
    /// is not one).
    pub links: usize,
    /// `{{…}}` templates dropped, nested ones counted once with their parent.
    pub templates_dropped: usize,
    /// Constructs never closed: a template, comment, tag, `[[` or table.
    pub unbalanced: usize,
}

impl WikitextStats {
    /// Add `other`'s counts to these (a sync sums its pages).
    pub fn add(&mut self, other: &WikitextStats) {
        self.headings += other.headings;
        self.code_blocks += other.code_blocks;
        self.inline_code += other.inline_code;
        self.links += other.links;
        self.templates_dropped += other.templates_dropped;
        self.unbalanced += other.unbalanced;
    }
}

/// Convert MediaWiki wikitext to markdown that keeps its headings and code
/// spans, with what was converted and dropped. The output ends with a newline
/// (or is empty) and no line carries trailing whitespace.
pub fn wikitext_to_markdown(src: &str) -> (String, WikitextStats) {
    let mut scan = Scan {
        src,
        out: String::with_capacity(src.len()),
        ..Scan::default()
    };
    scan.run();
    let Scan {
        out: text,
        inlines,
        blocks,
        mut stats,
        ..
    } = scan;
    let mut lines = Lines {
        inlines: &inlines,
        blocks: &blocks,
        stats: &mut stats,
        out: String::with_capacity(text.len()),
        pre_run: Vec::new(),
        table_depth: 0,
        row: Vec::new(),
    };
    for line in text.lines() {
        lines.line(line);
    }
    lines.finish();
    let mut md = lines.out;
    while md.ends_with("\n\n") {
        md.pop();
    }
    (md, stats)
}

// Placeholders: `INLINE_OPEN <n> INLINE_CLOSE` inside a line, and a line of
// `BLOCK_OPEN <n> BLOCK_CLOSE`. The scan drops these four chars from the input.
const INLINE_OPEN: char = '\u{E000}';
const INLINE_CLOSE: char = '\u{E001}';
const BLOCK_OPEN: char = '\u{E002}';
const BLOCK_CLOSE: char = '\u{E003}';

/// Formatting tags removed (open or close), their content kept.
#[rustfmt::skip]
const STRIP_TAGS: &[&str] = &[
    "abbr", "b", "big", "blockquote", "center", "cite", "del", "dfn", "div", "em", "font", "hr",
    "i", "ins", "kbd", "mark", "p", "q", "s", "samp", "small", "span", "strike", "strong", "sub",
    "sup", "u", "var",
];
/// Tags the scan handles itself.
const CODE_TAGS: &[&str] = &[
    "br",
    "code",
    "nowiki",
    "pre",
    "ref",
    "references",
    "source",
    "syntaxhighlight",
    "tt",
];
/// `__WORD__` behaviour switches, dropped.
#[rustfmt::skip]
const MAGIC_WORDS: &[&str] = &[
    "DISAMBIG", "EXPECTUNUSEDCATEGORY", "FORCETOC", "HIDDENCAT", "INDEX", "NEWSECTIONLINK",
    "NOCC", "NOCONTENTCONVERT", "NOEDITSECTION", "NOGALLERY", "NOINDEX", "NONEWSECTIONLINK",
    "NOTC", "NOTITLECONVERT", "NOTOC", "STATICREDIRECT", "TOC",
];
/// Link namespaces whose `[[…]]` is media or page metadata, not a link.
const DROP_NAMESPACES: &[&str] = &["category", "file", "image", "media"];
const URL_SCHEMES: &[&str] = &["http://", "https://", "ftp://", "ftps://", "mailto:"];
/// Wiki-link labels nested deeper than this are kept as written.
const MAX_LINK_DEPTH: usize = 4;

enum Inline {
    /// Inline code content, already cleaned.
    Code(String),
    /// `<nowiki>` content, kept as written.
    Literal(String),
}

struct Block {
    lang: String,
    body: String,
}

/// Pass one: drop templates, comments and refs; turn tags into placeholders.
#[derive(Default)]
struct Scan<'a> {
    src: &'a str,
    out: String,
    inlines: Vec<Inline>,
    blocks: Vec<Block>,
    stats: WikitextStats,
    /// Per close tag, the last `</name>` found (start, end), or `None` once a
    /// search found none. Searches run from increasing positions, so a cached
    /// close at or after the next search's start is its answer too, and a
    /// failed one stays failed: no stretch of the page is searched twice.
    closes: Vec<(&'static str, Option<(usize, usize)>)>,
    /// The last paragraph break found (a blank line's `\n`, or the page end).
    para_end: Option<usize>,
}

/// An HTML-style tag at the start of a string.
struct Tag<'a> {
    name: &'static str,
    close: bool,
    self_close: bool,
    attrs: &'a str,
    len: usize,
}

/// Parse `<name …>` / `</name>` / `<name/>` at the start of `s` when `name`
/// (any case) is in one of the `known` lists. The tag must close on its own
/// line and hold no other `<`.
fn parse_tag<'a>(s: &'a str, known: &[&[&'static str]]) -> Option<Tag<'a>> {
    let body = s.strip_prefix('<')?;
    let (close, body) = match body.strip_prefix('/') {
        Some(b) => (true, b),
        None => (false, body),
    };
    let name_len = body.bytes().take_while(u8::is_ascii_alphabetic).count();
    let name = known
        .iter()
        .flat_map(|list| list.iter().copied())
        .find(|k| k.eq_ignore_ascii_case(&body[..name_len]))?;
    let after = &body[name_len..];
    if !after.starts_with(|c: char| c == '>' || c == '/' || c.is_ascii_whitespace()) {
        return None;
    }
    let gt = after.find(['>', '<', '\n'])?;
    if !after[gt..].starts_with('>') {
        return None;
    }
    let attrs = after[..gt].trim();
    let self_close = attrs.ends_with('/');
    Some(Tag {
        name,
        close,
        self_close,
        attrs: attrs.trim_end_matches('/').trim_end(),
        len: s.len() - after.len() + gt + 1,
    })
}

/// The first `</name>` (any case, whitespace before `>`) in `s`: its start and end.
fn find_close(s: &str, name: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(p) = s[from..].find("</") {
        let start = from + p;
        let rest = &s.as_bytes()[start + 2..];
        if rest.len() >= name.len() && rest[..name.len()].eq_ignore_ascii_case(name.as_bytes()) {
            let tail = &rest[name.len()..];
            let ws = tail.iter().take_while(|b| b.is_ascii_whitespace()).count();
            if tail.get(ws) == Some(&b'>') {
                return Some((start, start + 2 + name.len() + ws + 1));
            }
        }
        from = start + 2;
    }
    None
}

/// The value of attribute `key` (`key="v"`, `key='v'` or `key=v`).
fn attr_value<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let mut rest = attrs;
    while let Some(p) = rest.find('=') {
        let name = rest[..p].trim_end();
        let name = name
            .rsplit(|c: char| c.is_ascii_whitespace())
            .next()
            .unwrap_or(name);
        let value = rest[p + 1..].trim_start();
        let (v, after) = match value.chars().next() {
            Some(q @ ('"' | '\'')) => match value[1..].find(q) {
                Some(e) => (&value[1..1 + e], &value[2 + e..]),
                None => (&value[1..], ""),
            },
            _ => {
                let e = value
                    .find(|c: char| c.is_ascii_whitespace())
                    .unwrap_or(value.len());
                (&value[..e], &value[e..])
            }
        };
        if name.eq_ignore_ascii_case(key) {
            return Some(v);
        }
        rest = after;
    }
    None
}

/// True when `attrs` holds the bare flag `key` (`<syntaxhighlight inline>`).
fn has_flag(attrs: &str, key: &str) -> bool {
    attrs
        .split_ascii_whitespace()
        .any(|w| w.eq_ignore_ascii_case(key))
}

impl Scan<'_> {
    fn run(&mut self) {
        let src = self.src;
        let mut i = 0;
        while let Some(c) = src[i..].chars().next() {
            let rest = &src[i..];
            let next = match c {
                '<' => self.at_tag(i),
                '{' if rest.starts_with("{{") => Some(self.template(i)),
                '_' if rest.starts_with("__") => magic_word(rest).map(|n| i + n),
                _ => None,
            };
            match next {
                Some(end) => {
                    i = end;
                    if !src[..i].ends_with('\n')
                        && (self.out.is_empty() || self.out.ends_with('\n'))
                    {
                        // A construct dropped at the start of a line, or a block
                        // with text after it on its closing line: that text is
                        // not a space-indented code line.
                        i += src[i..]
                            .bytes()
                            .take_while(|b| *b == b' ' || *b == b'\t')
                            .count();
                    }
                }
                None => {
                    if !(INLINE_OPEN..=BLOCK_CLOSE).contains(&c) {
                        self.out.push(c);
                    }
                    i += c.len_utf8();
                }
            }
        }
    }

    /// A `{{` at `i`: skip to its matching `}}`, or to the end when unbalanced.
    fn template(&mut self, i: usize) -> usize {
        self.stats.templates_dropped += 1;
        let b = self.src.as_bytes();
        let mut depth = 0usize;
        let mut j = i;
        while j + 1 < b.len() {
            match (b[j], b[j + 1]) {
                (b'{', b'{') => {
                    depth += 1;
                    j += 2;
                }
                (b'}', b'}') => {
                    depth -= 1;
                    j += 2;
                    if depth == 0 {
                        return j;
                    }
                }
                _ => j += 1,
            }
        }
        self.stats.unbalanced += 1;
        b.len()
    }

    /// A `<` at `i`: a comment or a known tag; the position after what it
    /// consumed, or `None` to keep the `<` as text.
    fn at_tag(&mut self, i: usize) -> Option<usize> {
        let src = self.src;
        let rest = &src[i..];
        if let Some(body) = rest.strip_prefix("<!--") {
            return Some(match body.find("-->") {
                Some(e) => i + 4 + e + 3,
                None => {
                    self.stats.unbalanced += 1;
                    src.len()
                }
            });
        }
        let tag = parse_tag(rest, &[CODE_TAGS, STRIP_TAGS])?;
        let after = i + tag.len;
        if tag.name == "br" {
            self.out.push(' ');
            return Some(after);
        }
        if tag.close || STRIP_TAGS.contains(&tag.name) {
            return Some(after);
        }
        if tag.self_close {
            if tag.name == "nowiki" {
                // `<nowiki/>` stops the next char opening a list or a link.
                self.push_inline(Inline::Literal(String::new()));
            }
            return Some(after);
        }
        let mut close = self.close_of(tag.name, after);
        if matches!(tag.name, "code" | "tt") {
            // A plain HTML tag: it cannot pair across a paragraph break (a
            // stray `<code>` must not swallow the headings after it).
            let para = self.paragraph_end(after);
            close = close.filter(|(s, _)| *s <= para);
        }
        // Extension tags (pre, source, syntaxhighlight) run to their close
        // or, unclosed, to the end of the page; the rest drop an unclosed tag.
        let verbatim = matches!(tag.name, "pre" | "source" | "syntaxhighlight");
        let (body, end) = match close {
            Some((s, e)) => (&src[after..s], e),
            None => {
                self.stats.unbalanced += 1;
                if !verbatim {
                    return Some(after);
                }
                (&src[after..], src.len())
            }
        };
        if !verbatim {
            match tag.name {
                "nowiki" => self.push_inline(Inline::Literal(one_line(&decode_entities(body)))),
                "code" | "tt" => self.push_inline(Inline::Code(clean_code(body))),
                _ => {} // ref, references: dropped with their content
            }
            return Some(end);
        }
        if tag.name != "pre" && has_flag(tag.attrs, "inline") {
            self.push_inline(Inline::Code(clean_code(body)));
            return Some(end);
        }
        let lang = if tag.name == "pre" {
            ""
        } else {
            attr_value(tag.attrs, "lang").unwrap_or("")
        };
        self.push_block(lang, body);
        // The block's placeholder line already ends with a newline.
        let newline = if src[end..].starts_with("\r\n") {
            2
        } else {
            usize::from(src[end..].starts_with('\n'))
        };
        Some(end + newline)
    }

    /// `</name>` at or after byte `from` (absolute), through the cache.
    fn close_of(&mut self, name: &'static str, from: usize) -> Option<(usize, usize)> {
        let cached = self.closes.iter().position(|(n, _)| *n == name);
        if let Some(p) = cached {
            match self.closes[p].1 {
                None => return None,
                Some((s, e)) if s >= from => return Some((s, e)),
                Some(_) => {}
            }
        }
        let found = find_close(&self.src[from..], name).map(|(s, e)| (from + s, from + e));
        match cached {
            Some(p) => self.closes[p].1 = found,
            None => self.closes.push((name, found)),
        }
        found
    }

    /// The `\n` that opens the first blank line at or after `from`, or the
    /// page's length.
    fn paragraph_end(&mut self, from: usize) -> usize {
        if let Some(p) = self.para_end.filter(|p| *p >= from) {
            return p;
        }
        let src = self.src;
        let b = src.as_bytes();
        let mut end = src.len();
        let mut j = from;
        while let Some(k) = src[j..].find('\n') {
            let nl = j + k;
            let ws = b[nl + 1..]
                .iter()
                .take_while(|c| matches!(c, b' ' | b'\t' | b'\r'))
                .count();
            if b.get(nl + 1 + ws) == Some(&b'\n') {
                end = nl;
                break;
            }
            j = nl + 1;
        }
        self.para_end = Some(end);
        end
    }

    fn push_inline(&mut self, tok: Inline) {
        self.out.push(INLINE_OPEN);
        self.out.push_str(&self.inlines.len().to_string());
        self.out.push(INLINE_CLOSE);
        self.inlines.push(tok);
    }

    fn push_block(&mut self, lang: &str, body: &str) {
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.out.push(BLOCK_OPEN);
        self.out.push_str(&self.blocks.len().to_string());
        self.out.push(BLOCK_CLOSE);
        self.out.push('\n');
        let lang = lang
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '_' | '.' | '#'))
            .collect();
        self.blocks.push(Block {
            lang,
            body: decode_entities(body),
        });
    }
}

/// The length of a `__MAGICWORD__` at the start of `s`.
fn magic_word(s: &str) -> Option<usize> {
    let body = s.get(2..)?;
    let n = body
        .bytes()
        .take(32)
        .take_while(u8::is_ascii_uppercase)
        .count();
    (body[n..].starts_with("__") && MAGIC_WORDS.contains(&&body[..n])).then_some(n + 4)
}

/// Inline code content: `<nowiki>` tags removed, entities decoded, one line, trimmed.
fn clean_code(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while let Some(c) = s[i..].chars().next() {
        if c == '<'
            && let Some(tag) = parse_tag(&s[i..], &[&["nowiki"]])
        {
            i += tag.len;
            continue;
        }
        out.push(c);
        i += c.len_utf8();
    }
    one_line(&decode_entities(&out)).trim().to_string()
}

fn one_line(s: &str) -> String {
    s.replace(['\r', '\n', '\t'], " ")
}

/// Decode `&lt;` `&gt;` `&amp;` `&quot;` `&apos;` `&nbsp;` and numeric
/// references (a control char stays encoded).
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        let tail = &rest[p..];
        match entity(tail) {
            Some((c, n)) => {
                out.push(c);
                rest = &tail[n..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn entity(s: &str) -> Option<(char, usize)> {
    let semi = s.bytes().take(12).position(|b| b == b';')?;
    let c = match &s[1..semi] {
        "lt" => '<',
        "gt" => '>',
        "amp" => '&',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        name => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
                .filter(|c| !c.is_control() && !(INLINE_OPEN..=BLOCK_CLOSE).contains(c))?
        }
    };
    Some((c, semi + 1))
}

/// Pass two: line by line, over the scan's text.
struct Lines<'a> {
    inlines: &'a [Inline],
    blocks: &'a [Block],
    stats: &'a mut WikitextStats,
    out: String,
    /// The space-indented code run in progress, leading space removed.
    pre_run: Vec<String>,
    table_depth: usize,
    /// The table row in progress, rendered cells.
    row: Vec<String>,
}

impl Lines<'_> {
    fn line(&mut self, raw: &str) {
        let blocks = self.blocks;
        if let Some(b) = block_index(raw).and_then(|n| blocks.get(n)) {
            self.end_pre();
            self.flush_row();
            let body: Vec<&str> = b.body.lines().collect();
            self.fence(&b.lang, &body);
            return;
        }
        let t = raw.trim_start();
        if self.table_depth > 0 || t.starts_with("{|") {
            self.end_pre();
            self.table_line(t);
            return;
        }
        if let Some(code) = raw.strip_prefix(' ')
            && (!code.trim().is_empty() || !self.pre_run.is_empty())
        {
            let code = self.expand(&decode_entities(code), true);
            self.pre_run.push(code);
            return;
        }
        self.end_pre();
        if t.is_empty() {
            self.emit("");
        } else if let Some((level, inner)) = heading(raw) {
            let text = self.render(inner);
            if !text.is_empty() {
                self.stats.headings += 1;
                self.emit(&format!("{} {text}", "#".repeat(level)));
            }
        } else if raw.starts_with("----") {
            self.emit("");
            self.emit("---");
            let text = self.render(raw.trim_start_matches('-'));
            self.emit_text("", &text);
        } else if raw
            .get(..9)
            .is_some_and(|p| p.eq_ignore_ascii_case("#redirect"))
        {
            let text = self.render(&raw[9..]);
            self.emit_text("", &format!("REDIRECT {text}"));
        } else {
            self.list_or_paragraph(raw);
        }
    }

    fn list_or_paragraph(&mut self, raw: &str) {
        let plen = raw
            .bytes()
            .take_while(|b| matches!(b, b'*' | b'#' | b':' | b';'))
            .count();
        let text = self.render(&raw[plen..]);
        let Some(&last) = raw.as_bytes()[..plen].last() else {
            self.emit_text("", &text);
            return;
        };
        let mut prefix = String::new();
        for (k, b) in raw.bytes().take(plen).enumerate() {
            if k + 1 < plen || matches!(b, b':' | b';') {
                prefix.push_str(if b == b'#' { "   " } else { "  " });
            }
        }
        prefix.push_str(match last {
            b'*' => "- ",
            b'#' => "1. ",
            _ => "",
        });
        self.emit_text(&prefix, &text);
    }

    fn table_line(&mut self, t: &str) {
        if t.starts_with("{|") {
            self.flush_row();
            self.table_depth += 1;
        } else if t.starts_with("|}") {
            self.flush_row();
            self.table_depth = self.table_depth.saturating_sub(1);
        } else if let Some(caption) = t.strip_prefix("|+") {
            self.flush_row();
            let text = self.render(cell_content(caption));
            self.emit_text("", &text);
        } else if t.starts_with("|-") {
            self.flush_row();
        } else if let Some((cells, header)) = t
            .strip_prefix('!')
            .map(|r| (r, true))
            .or_else(|| t.strip_prefix('|').map(|r| (r, false)))
        {
            for cell in split_cells(cells, header) {
                let text = self.render(cell_content(cell));
                self.row.push(text);
            }
        } else if !t.trim().is_empty() {
            let text = self.render(t);
            match self.row.last_mut() {
                Some(cell) if !cell.is_empty() => {
                    cell.push(' ');
                    cell.push_str(&text);
                }
                Some(cell) => *cell = text,
                None => self.row.push(text),
            }
        }
    }

    fn flush_row(&mut self) {
        let cells: Vec<String> = self.row.drain(..).filter(|c| !c.is_empty()).collect();
        if !cells.is_empty() {
            self.emit_text("", &cells.join(" | "));
        }
    }

    fn end_pre(&mut self) {
        while self.pre_run.last().is_some_and(|l| l.trim().is_empty()) {
            self.pre_run.pop();
        }
        if self.pre_run.is_empty() {
            return;
        }
        let run = std::mem::take(&mut self.pre_run);
        let body: Vec<&str> = run.iter().map(String::as_str).collect();
        self.fence("", &body);
    }

    fn finish(&mut self) {
        self.end_pre();
        self.flush_row();
        if self.table_depth > 0 {
            self.stats.unbalanced += 1;
        }
    }

    /// A fenced block of `body`, leading and trailing blank lines dropped. The
    /// fence is one no body line opens with, so the engine's chunker (which
    /// closes a fence on any line starting with its marker) sees it whole.
    fn fence(&mut self, lang: &str, body: &[&str]) {
        let first = body.iter().position(|l| !l.trim().is_empty());
        let last = body.iter().rposition(|l| !l.trim().is_empty());
        let (Some(first), Some(last)) = (first, last) else {
            return;
        };
        let body = &body[first..=last];
        let opens = |m: &str| body.iter().any(|l| l.trim_start().starts_with(m));
        let fence = if !opens("```") {
            "```".to_string()
        } else if !opens("~~~") {
            "~~~".to_string()
        } else {
            let run = body
                .iter()
                .map(|l| l.trim_start().bytes().take_while(|b| *b == b'`').count());
            "`".repeat(run.max().unwrap_or(0) + 1)
        };
        self.stats.code_blocks += 1;
        for line in std::iter::once(format!("{fence}{lang}"))
            .chain(body.iter().map(|l| l.trim_end().to_string()))
            .chain(std::iter::once(fence.clone()))
        {
            self.out.push_str(&line);
            self.out.push('\n');
        }
    }

    /// One output line, trailing whitespace trimmed; blank lines never repeat
    /// and never open the page.
    fn emit(&mut self, line: &str) {
        let line = line.trim_end();
        if line.is_empty() {
            if !self.out.is_empty() && !self.out.ends_with("\n\n") {
                self.out.push('\n');
            }
            return;
        }
        self.out.push_str(line);
        self.out.push('\n');
    }

    /// A text line after `prefix`, skipped when empty. Text that would open a
    /// line with `#`, ```` ``` ```` or `~~~` is escaped, so the chunker never
    /// reads it as a heading or a fence.
    fn emit_text(&mut self, prefix: &str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let escape = prefix.trim().is_empty()
            && (text.starts_with('#') || text.starts_with("```") || text.starts_with("~~~"));
        self.emit(&format!("{prefix}{}{text}", if escape { "\\" } else { "" }));
    }

    /// Inline markup converted, entities decoded, whitespace collapsed, then
    /// the placeholders expanded (so code spans keep their spacing).
    fn render(&mut self, s: &str) -> String {
        let converted = inline(s, self.stats, 0);
        let collapsed = decode_entities(&converted)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        self.expand(&collapsed, false)
    }

    /// Replace inline placeholders: code as a backtick span (`raw`: as its
    /// bare content, inside a code block), `<nowiki>` text as written.
    fn expand(&mut self, s: &str, raw: bool) -> String {
        let mut out = String::with_capacity(s.len());
        let mut rest = s;
        while let Some(p) = rest.find(INLINE_OPEN) {
            out.push_str(&rest[..p]);
            let tail = &rest[p + INLINE_OPEN.len_utf8()..];
            let q = tail.find(INLINE_CLOSE).unwrap_or(tail.len());
            match tail[..q]
                .parse::<usize>()
                .ok()
                .and_then(|n| self.inlines.get(n))
            {
                Some(Inline::Code(c)) if raw => out.push_str(c),
                Some(Inline::Code(c)) if !c.is_empty() => {
                    self.stats.inline_code += 1;
                    out.push_str(&code_span(c));
                }
                Some(Inline::Literal(t)) => out.push_str(t),
                _ => {}
            }
            rest = tail.get(q + INLINE_CLOSE.len_utf8()..).unwrap_or("");
        }
        out.push_str(rest);
        out
    }
}

/// The block index of a line that is exactly one block placeholder.
fn block_index(line: &str) -> Option<usize> {
    line.strip_prefix(BLOCK_OPEN)?
        .strip_suffix(BLOCK_CLOSE)?
        .parse()
        .ok()
}

/// `== H ==` → (2, " H "). The level is the shorter `=` run, at most six; the
/// rest of a longer run stays in the text, as MediaWiki reads it.
fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_end();
    let lead = t.bytes().take_while(|b| *b == b'=').count();
    if lead == t.len() {
        return None;
    }
    let trail = t.bytes().rev().take_while(|b| *b == b'=').count();
    let level = lead.min(trail).min(6);
    (level > 0).then(|| (level, &t[level..t.len() - level]))
}

/// `x` as a backtick code span: the fence is one longer than the longest
/// backtick run inside, padded when `x` opens or closes with a backtick.
fn code_span(x: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in x.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(longest + 1);
    let pad = if x.starts_with('`') || x.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{x}{pad}{fence}")
}

/// A table line's cells: split on `||` (and `!!` in a header line) outside
/// `[[…]]`.
fn split_cells(s: &str, header: bool) -> Vec<&str> {
    let b = s.as_bytes();
    let (mut depth, mut start, mut j) = (0usize, 0, 0);
    let mut cells = Vec::new();
    while j + 1 < b.len() {
        match (b[j], b[j + 1]) {
            (b'[', b'[') => depth += 1,
            (b']', b']') => depth = depth.saturating_sub(1),
            (b'|', b'|') | (b'!', b'!') if depth == 0 && (header || b[j] == b'|') => {
                cells.push(&s[start..j]);
                start = j + 2;
            }
            _ => {
                j += 1;
                continue;
            }
        }
        j += 2;
    }
    cells.push(&s[start..]);
    cells
}

/// A cell's content: the text after its attributes (`style="…" | text`), the
/// first single `|` outside `[[…]]`.
fn cell_content(cell: &str) -> &str {
    let b = cell.as_bytes();
    let mut depth = 0usize;
    let mut j = 0;
    while j < b.len() {
        if b[j..].starts_with(b"[[") {
            depth += 1;
            j += 2;
        } else if b[j..].starts_with(b"]]") {
            depth = depth.saturating_sub(1);
            j += 2;
        } else if b[j] == b'|' && depth == 0 {
            return &cell[j + 1..];
        } else {
            j += 1;
        }
    }
    cell
}

/// Each `[[` in `s` paired with its matching `]]`, by position (one linear
/// pass; an unmatched `[[` has no entry).
fn link_pairs(s: &str) -> HashMap<usize, usize> {
    let b = s.as_bytes();
    let mut open = Vec::new();
    let mut pairs = HashMap::new();
    let mut j = 0;
    while j + 1 < b.len() {
        match (b[j], b[j + 1]) {
            (b'[', b'[') => open.push(j),
            (b']', b']') => {
                if let Some(o) = open.pop() {
                    pairs.insert(o, j);
                }
            }
            _ => {
                j += 1;
                continue;
            }
        }
        j += 2;
    }
    pairs
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// Inline markup in one line: emphasis, wiki links, external links.
/// Emphasis still open at the end of the line is closed there, as MediaWiki does.
fn inline(s: &str, stats: &mut WikitextStats, depth: usize) -> String {
    let pairs = link_pairs(s);
    let mut out = String::with_capacity(s.len());
    let mut open: Vec<&'static str> = Vec::new();
    let mut no_bracket = false;
    let mut i = 0;
    while let Some(c) = s[i..].chars().next() {
        let rest = &s[i..];
        if rest.starts_with("''") {
            let n = rest.bytes().take_while(|b| *b == b'\'').count();
            let (literal, marks): (usize, &[&'static str]) = match n {
                2 => (0, &["*"]),
                3 => (0, &["**"]),
                4 => (1, &["**"]),
                _ => (n - 5, &["**", "*"]),
            };
            out.extend(std::iter::repeat_n('\'', literal));
            for &m in marks {
                match open.iter().rposition(|o| *o == m) {
                    Some(p) => {
                        open.remove(p);
                    }
                    None => open.push(m),
                }
                out.push_str(m);
            }
            i += n;
            continue;
        }
        if rest.starts_with("[[") {
            match pairs.get(&i) {
                Some(&close) => {
                    out.push_str(&wiki_link(&s[i + 2..close], stats, depth));
                    i = close + 2;
                }
                None => {
                    stats.unbalanced += 1;
                    out.push_str("[[");
                    i += 2;
                }
            }
            continue;
        }
        if c == '[' && !no_bracket && URL_SCHEMES.iter().any(|p| starts_with_ci(&rest[1..], p)) {
            match rest.find(']') {
                Some(close) => {
                    out.push_str(&external_link(&rest[1..close], stats, depth));
                    i += close + 1;
                    continue;
                }
                None => no_bracket = true,
            }
        }
        out.push(c);
        i += c.len_utf8();
    }
    while let Some(m) = open.pop() {
        out.push_str(m);
    }
    out
}

/// `Target|label` → the label, `Target` → the target; a File / Category
/// link → nothing (`:Category:X`, a link to the category, is kept).
fn wiki_link(inner: &str, stats: &mut WikitextStats, depth: usize) -> String {
    let (target, label) = match inner.split_once('|') {
        Some((t, l)) => (t.trim(), Some(l.trim())),
        None => (inner.trim(), None),
    };
    let target = match target.strip_prefix(':') {
        Some(t) => t,
        None => {
            let ns = target.split_once(':').map(|(ns, _)| ns.trim());
            if ns.is_some_and(|ns| DROP_NAMESPACES.iter().any(|d| d.eq_ignore_ascii_case(ns))) {
                return String::new();
            }
            target
        }
    };
    stats.links += 1;
    match label.filter(|l| !l.is_empty()) {
        Some(l) if depth < MAX_LINK_DEPTH => inline(l, stats, depth + 1),
        Some(l) => l.to_string(),
        None => target.to_string(),
    }
}

/// `https://u label` → `[label](https://u)`; a bare `https://u` stays a URL.
fn external_link(inner: &str, stats: &mut WikitextStats, depth: usize) -> String {
    stats.links += 1;
    let (url, label) = match inner.find(char::is_whitespace) {
        Some(p) => (&inner[..p], inner[p..].trim()),
        None => (inner, ""),
    };
    if label.is_empty() {
        return url.to_string();
    }
    let label = if depth < MAX_LINK_DEPTH {
        inline(label, stats, depth + 1)
    } else {
        label.to_string()
    };
    format!("[{label}]({url})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(src: &str) -> String {
        wikitext_to_markdown(src).0
    }

    #[test]
    fn headings_and_code_survive() {
        let src = "== Orders ==\nCall <code>OrderService.place</code>.\n<syntaxhighlight lang=\"python\">\nsvc.place(o)\n</syntaxhighlight>";
        let (out, stats) = wikitext_to_markdown(src);
        assert_eq!(
            out,
            "## Orders\nCall `OrderService.place`.\n```python\nsvc.place(o)\n```\n"
        );
        assert_eq!(
            (stats.headings, stats.code_blocks, stats.inline_code),
            (1, 1, 1)
        );
    }

    #[test]
    fn templates_and_refs_are_dropped() {
        let (out, stats) = wikitext_to_markdown("a {{Infobox|x={{nested}}}} b<ref>c</ref> d");
        assert_eq!(out, "a b d\n");
        assert_eq!((stats.templates_dropped, stats.unbalanced), (1, 0));
        let multi = "{{Infobox\n| name = svc\n| lang = {{lang|go}}\n}}\nIntro <ref name=\"r\"/>text.<!-- note\nspanning -->\n__TOC__\n";
        assert_eq!(md(multi), "Intro text.\n");
    }

    #[test]
    fn links_and_lists() {
        let (out, stats) =
            wikitext_to_markdown("* see [[Order Flow|the flow]] and [https://x.test docs]");
        assert_eq!(out, "- see the flow and [docs](https://x.test)\n");
        assert_eq!(stats.links, 2);
        let nested = "# one\n## two [[Page]]\n#* three\n: indented\n;term\n[[File:x.png|thumb|cap]][[Category:Ops]]\n[https://bare.test]";
        assert_eq!(
            md(nested),
            "1. one\n   1. two Page\n   - three\n  indented\n  term\nhttps://bare.test\n"
        );
    }

    #[test]
    fn never_panics() {
        let mut sample = String::new();
        while sample.len() < 2048 {
            sample.push_str(
                "= Überblick =\n''Grüße'' '''日本語''' [[Seite|Link]] {{Vorlage|a={{b}}}}\n\
                 * <code>a`b</code> <tt>x</tt> [https://ex.test/ä ラベル] <nowiki>[[x]]</nowiki>\n\
                 {| class=\"t\"\n! H1 !! H2\n|-\n| α || [[B|β]]\n|}\n <!-- c --> pre ✓\n",
            );
        }
        sample.push_str("{{open <pre>\n== not a heading ==\n");
        let mut checked = 0;
        for (k, _) in sample
            .char_indices()
            .chain(std::iter::once((sample.len(), ' ')))
        {
            let (out, _) = wikitext_to_markdown(&sample[..k]);
            assert!(out.is_empty() || out.ends_with('\n'), "prefix {k}");
            assert!(
                out.lines().all(|l| l == l.trim_end()),
                "trailing space at prefix {k}"
            );
            checked += 1;
        }
        assert!(sample.len() > 2048);
        assert_eq!(checked, sample.chars().count() + 1);
        // The whole sample leaves one template open: dropped to the end, counted.
        let (out, stats) = wikitext_to_markdown(&sample);
        assert_eq!(stats.unbalanced, 1);
        assert!(!out.contains("not a heading"));
        // Cut inside the first template's inner `{{b}}`: open, dropped, counted.
        let cut = sample.find("{{b").expect("inner template") + 3;
        let (_, stats) = wikitext_to_markdown(&sample[..cut]);
        assert_eq!((stats.templates_dropped, stats.unbalanced), (1, 1));
        // Cut inside a `<code>` / `<pre>` / comment: each counted once.
        for (src, n) in [
            ("x <code>y", 1),
            ("<pre>\nz", 1),
            ("a <!-- b", 1),
            ("[[x", 1),
        ] {
            assert_eq!(wikitext_to_markdown(src).1.unbalanced, n, "{src:?}");
        }
        assert_eq!(md("<pre>\nz"), "```\nz\n```\n");
    }

    #[test]
    fn code_blocks_are_verbatim_and_fence_safe() {
        // A heading or `#` comment inside a code block stays code.
        let src = "<pre>\n== Not ==\n{{kept}}\n</pre>\n<source lang=\"bash\">\n# install\nmake &amp;&amp; make test\n</source>";
        assert_eq!(
            md(src),
            "```\n== Not ==\n{{kept}}\n```\n```bash\n# install\nmake && make test\n```\n"
        );
        // A body line opening with ``` gets a ~~~ fence.
        assert_eq!(
            md("<syntaxhighlight lang=\"md\">\n```\nx\n```\n</syntaxhighlight>"),
            "~~~md\n```\nx\n```\n~~~\n"
        );
        // A space-indented run is one block; a template dropped at line start
        // does not make one.
        // (A line of one space continues the run; an empty line ends it, as in
        // MediaWiki.)
        let (out, stats) =
            wikitext_to_markdown("Run:\n make\n \n make test\nafter\n{{t}} not code");
        assert_eq!(out, "Run:\n```\nmake\n\nmake test\n```\nafter\nnot code\n");
        assert_eq!(stats.code_blocks, 1);
        // A block's closing line does not eat the next line's indent.
        assert_eq!(md("<pre>a</pre>\n b"), "```\na\n```\n```\nb\n```\n");
        // A stray <code> does not pair across a paragraph break and swallow a heading.
        let (out, stats) = wikitext_to_markdown("x <code>open\n\n== Next ==\nuse <code>y</code>");
        assert_eq!(out, "x open\n\n## Next\nuse `y`\n");
        assert_eq!(
            (stats.headings, stats.inline_code, stats.unbalanced),
            (1, 1, 1)
        );
        // Inline syntaxhighlight is an inline span.
        assert_eq!(
            md("use <syntaxhighlight lang=\"go\" inline>x := 1</syntaxhighlight> here"),
            "use `x := 1` here\n"
        );
    }

    #[test]
    fn inline_markup() {
        assert_eq!(
            md("'''b''' and ''i'' and '''''bi'''''"),
            "**b** and *i* and ***bi***\n"
        );
        assert_eq!(md("''open"), "*open*\n");
        assert_eq!(md("<code>a`b</code> <code></code>"), "``a`b``\n");
        assert_eq!(
            md("<nowiki>''[[x]]''</nowiki> <code><nowiki>{{t}}</nowiki></code>"),
            "''[[x]]'' `{{t}}`\n"
        );
        assert_eq!(md("<nowiki/>* not a list"), "* not a list\n");
        assert_eq!(
            md("x<br/>y <span style=\"c\">z</span> a &lt; b"),
            "x y z a < b\n"
        );
        assert_eq!(md(": # hash\n<nowiki>#</nowiki> x"), "  \\# hash\n\\# x\n");
        assert_eq!(
            md("[[:Category:Ops]] [[Svc#Api]]"),
            "Category:Ops Svc#Api\n"
        );
        assert_eq!(md("text\n----\nmore"), "text\n\n---\nmore\n");
    }

    #[test]
    fn tables_flatten_to_rows() {
        let src = "{| class=\"wikitable\"\n|+ Services\n! Name !! Port\n|-\n| style=\"x\" | <code>api</code> || 8080\n|-\n| [[Worker|worker]]\n| 9090\ncontinued\n|}\nafter";
        let (out, stats) = wikitext_to_markdown(src);
        assert_eq!(
            out,
            "Services\nName | Port\n`api` | 8080\nworker | 9090 continued\nafter\n"
        );
        assert_eq!(
            (stats.inline_code, stats.links, stats.unbalanced),
            (1, 1, 0)
        );
        assert_eq!(wikitext_to_markdown("{|\n| a").1.unbalanced, 1);
    }

    /// Repeated unclosed constructs on one 200 KB line: a search that restarted
    /// at each one would be quadratic (4e10 steps); the caches keep it linear.
    #[test]
    fn stays_linear_on_pathological_input() {
        for unit in [
            "<code>",
            "<ref>",
            "<nowiki>",
            "<span ",
            "[[a|",
            "[[File:",
            "[https://x ",
            "''''''",
            "{{",
            "<!--",
            "= ",
            "|| ",
        ] {
            let src = unit.repeat(200_000 / unit.len());
            let (out, _) = wikitext_to_markdown(&src);
            assert!(out.len() <= src.len() * 3, "{unit:?}");
            let lines = format!("\n{unit}").repeat(20_000);
            wikitext_to_markdown(&lines);
        }
    }
}
