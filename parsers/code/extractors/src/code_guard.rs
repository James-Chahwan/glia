//! CJ.1a: the per-file literal / comment guard every needle scanner shares.
//!
//! A needle scanner reads a call site off raw text, so a needle that sits in a
//! string literal or a comment (a scanner's own needle table, a doc comment, a
//! test's embedded sample source) used to mint a node as if it were a call.
//! In a file of a language [`lexer_for`] names this guard refuses an
//! occurrence whose FIRST byte is inside a literal or comment, lexed by
//! LA.20a's [`CodeMap`] (the CLI extractor's rule: "a needle in a doc comment
//! or a string is not a declaration"). Only the needle's first byte is
//! checked: every queue and event needle starts in code at a real site, while
//! the topic literal it reads sits after it.
//!
//! Scope: Rust (`.rs`) and Python (`.py`, `.pyw`, `.pyi`), the languages glia
//! and its tests are written in (CJ.1a), and since FD.5a Ruby (`.rb`, `.rake`,
//! `.gemspec`), PHP (`.php`), Dart (`.dart`) and Elixir (`.ex`, `.exs`),
//! lexed by [`CodeMap::script`] (DH.10). FD.5b adds the C family and the JVM /
//! JS family: Go (`.go`), Kotlin (`.kt`, `.kts`), Scala (`.scala`, `.sc`),
//! Swift (`.swift`) and JavaScript / TypeScript (`.js`, `.jsx`, `.mjs`,
//! `.cjs`, `.ts`, `.tsx`, `.mts`, `.cts`) through their own
//! [`CodeMap::script`] arms, and Java (`.java`), C# (`.cs`), C (`.c`, `.h`)
//! and C++ (`.cc`, `.cpp`, `.cxx`, `.hpp`, `.hh`) through [`CodeMap::new`].
//! A `.vue` / `.svelte` / `.html` file has no guard: its text outside
//! `<script>` is not JS, so its scans stay as they were. Every other file has
//! no guard either.
//!
//! Interpolation. A Python f-string is read as code whole: its `{..}`
//! replacement fields are expressions (`f"redis://{os.getenv('REDIS_HOST')}"`),
//! so an f-string keeps the behaviour it had before the guard, while a plain,
//! raw, byte or triple-quoted string and every comment are refused. Rust needs
//! no such rule: `format!` arguments sit outside the literal and an inline
//! `{x}` holds an identifier only. Ruby and Elixir `#{..}`, PHP `{$..}` /
//! `${..}` and Dart `${..}` interpolations are code too, but in those
//! languages only the interpolation span is: a needle whose first byte sits
//! inside an interpolating literal (Ruby and PHP double-quoted strings and
//! heredocs that are not `'ID'`-quoted, Elixir double- and single-quoted
//! strings and `"""` / `'''` heredocs, Dart strings that are not raw `r'..'`)
//! counts as code ONLY inside a `#{..}` / `${..}` / `{$..}` span. That is
//! stricter than Python's whole-literal rule on purpose: these languages
//! delimit their code spans, so the literal text around one is text (a
//! channel name built as `"orders.#{region}"` keeps its reading: its needle,
//! the call, starts in code before the literal). Python keeps the whole
//! f-string rule because fixtures read through it and an f-string's field
//! is not delimited by a sigil the old scans knew. A Dart `$name` or PHP
//! `"$x"` simple interpolation holds a name only, never a call, so it is text.
//! Elixir sigils (`~s(..)`) are read as code by [`CodeMap::script`], so their
//! content is still scanned.
//!
//! FD.5b interpolation: a JS template literal's `${..}`, a Kotlin string's
//! `${..}` (every Kotlin string interpolates), a Scala interpolator's
//! (`s".."`, `f".."`, any `id".."`) `${..}` and a Swift `\(..)` (`\#(..)` in
//! a raw `#".."#`) are code, mapped as code by the lexer itself; the literal
//! text around a span is text. A Kotlin / Scala `$name` holds a name only, so
//! it is text, as in Dart. Go raw strings (backticks) and Java / C# / C / C++
//! strings do not interpolate. A C# `$".."` string's `{..}` holes are text:
//! [`CodeMap::new`] reads it as a plain string. JS regex literals are not
//! lexed ([`Script::Js`]).
//!
//! Statement literals (FD.4b, [`CodeGuard::opens_argument`]): the call
//! argument rule applies to Rust and Python only. In the FD.5a and FD.5b
//! languages (a Go `const q = "SELECT .."`, a JS ``const q = `SELECT ..` ``,
//! a Kotlin `val q = """SELECT .."""` are as common as Ruby's) a
//! SQL / Cypher statement literal is refused only when it does not open in
//! code (it sits in a comment or another literal); a statement assigned to a
//! constant (`SQL = "SELECT .."`, common in Ruby and PHP) keeps today's
//! reading.
//!
//! fired_on marker, once per scanner call that refused an occurrence:
//!   `... 2>&1 | grep '\[code-guard\]'`
//!   `[code-guard] <scanner> lang=<rust|python|ruby|php|dart|elixir|go|java|kotlin|scala|csharp|swift|c|cpp|javascript|typescript> dropped=<n> path=<path>`
//!
//! Library names (FD.5a, [`CodeGuard::is_code_or_script_string`]): in every
//! guarded language but Rust and Python a name in a string literal is read
//! (an import string, a connection URL); only a comment refuses one.

use crate::cli::{CodeMap, Script};

/// The lexer a file's path selects, by its LAST extension (ASCII
/// case-insensitive): the language tag the marker prints and the
/// [`CodeMap::script`] syntax (`None` = [`CodeMap::new`], the C family's
/// rules, which cover Rust's). `None` for every other file and an empty path.
fn lexer_for(path: &str) -> Option<(&'static str, Option<Script>)> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (_, ext) = name.rsplit_once('.')?;
    let is = |exts: &[&str]| exts.iter().any(|e| ext.eq_ignore_ascii_case(e));
    if is(&["rs"]) {
        Some(("rust", None))
    } else if is(&["py", "pyw", "pyi"]) {
        Some(("python", Some(Script::Python)))
    } else if is(&["rb", "rake", "gemspec"]) {
        Some(("ruby", Some(Script::Ruby)))
    } else if is(&["php"]) {
        Some(("php", Some(Script::Php)))
    } else if is(&["dart"]) {
        Some(("dart", Some(Script::Dart)))
    } else if is(&["ex", "exs"]) {
        Some(("elixir", Some(Script::Elixir)))
    } else if is(&["go"]) {
        Some(("go", Some(Script::Go)))
    } else if is(&["java"]) {
        Some(("java", None))
    } else if is(&["kt", "kts"]) {
        Some(("kotlin", Some(Script::Kotlin)))
    } else if is(&["scala", "sc"]) {
        Some(("scala", Some(Script::Scala)))
    } else if is(&["cs"]) {
        Some(("csharp", None))
    } else if is(&["swift"]) {
        Some(("swift", Some(Script::Swift)))
    } else if is(&["c", "h"]) {
        Some(("c", None))
    } else if is(&["cc", "cpp", "cxx", "hpp", "hh"]) {
        Some(("cpp", None))
    } else if is(&["js", "jsx", "mjs", "cjs"]) {
        Some(("javascript", Some(Script::Js)))
    } else if is(&["ts", "tsx", "mts", "cts"]) {
        Some(("typescript", Some(Script::Js)))
    } else {
        None
    }
}

/// How far back [`CodeGuard::opens_argument`] looks for the `(` of the call
/// a `,`-separated argument sits in.
const ARG_WINDOW: usize = 8192;

/// Keywords a `(` can follow that open no call (Rust and Python alike).
const NOT_CALLEES: &[&str] = &[
    "and", "assert", "await", "elif", "else", "for", "if", "in", "is", "let", "match", "mut",
    "not", "or", "return", "while", "yield",
];

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// One file's literal / comment map, for the languages [`lexer_for`] names.
pub(crate) struct CodeGuard<'a> {
    map: CodeMap,
    src: &'a [u8],
    lang: &'static str,
    /// FD.6: the byte spans of a Rust file's `#[cfg(test)]` / `#[test]`
    /// items ([`CodeGuard::in_rust_test`]), found on first use.
    test_spans: std::cell::OnceCell<Vec<(usize, usize)>>,
}

impl<'a> CodeGuard<'a> {
    /// The guard for `source` read from `path`, or `None` when the file's
    /// language has none (every scan stays as it was).
    pub(crate) fn for_path(path: &str, source: &'a str) -> Option<Self> {
        let (lang, script) = lexer_for(path)?;
        let map = match script {
            None => CodeMap::new(source),
            Some(script) => CodeMap::script(source, script),
        };
        Some(CodeGuard {
            map,
            src: source.as_bytes(),
            lang,
            test_spans: std::cell::OnceCell::new(),
        })
    }

    /// The first byte of the literal or comment holding `pos` (its opening
    /// quote, its `//` / `#`), or `None` when `pos` is code.
    pub(crate) fn literal_start(&self, pos: usize) -> Option<usize> {
        self.map.literal_start(pos)
    }

    /// CJ.1c: true when a literal opens AT `pos` (its opening quote is byte
    /// `pos`). A quoted-key needle (Celery beat's `'schedule':`) is a real
    /// dict key only when the literal holding it starts at the needle; inside
    /// a larger string, a comment or (Rust) a char-less `'..'` it is not.
    pub(crate) fn opens_literal(&self, pos: usize) -> bool {
        self.literal_start(pos) == Some(pos)
    }

    /// `rust`, `python`, `ruby`, `php`, `dart`, `elixir`, or (FD.5b) `go`,
    /// `java`, `kotlin`, `scala`, `csharp`, `swift`, `c`, `cpp`,
    /// `javascript`, `typescript`.
    pub(crate) fn lang(&self) -> &'static str {
        self.lang
    }

    /// True when byte `pos` may start a call site: it is code, (Python) it
    /// sits in an f-string, whose replacement fields are code, or (Ruby, PHP,
    /// Dart, Elixir) it sits inside an interpolation span of an
    /// interpolating literal ([`CodeGuard::in_interpolation`]). FD.5b: the
    /// JS, Go, Kotlin, Scala and Swift maps hold a literal's text pieces
    /// only, so a `${..}` / `\(..)` span is code in the map itself.
    pub(crate) fn is_code(&self, pos: usize) -> bool {
        match self.literal_start(pos) {
            None => true,
            Some(open) if self.lang == "python" => self.is_fstring(open),
            Some(open) => self.in_interpolation(open, pos),
        }
    }

    /// FD.5a: true when byte `pos` is code ([`CodeGuard::is_code`]) or, in a
    /// file of any guarded language but Rust and Python (FD.5b adds the
    /// compiled and JS languages), sits in a string literal (not a
    /// comment). Those languages name a library in an import string
    /// (`require 'redis'`, `import 'package:redis/redis.dart'`, PHP
    /// `require_once 'redis.php'`), so a scanner that reads library
    /// names keeps those reads; Rust and Python import in code, so for them
    /// this is [`CodeGuard::is_code`].
    pub(crate) fn is_code_or_script_string(&self, pos: usize) -> bool {
        if self.is_code(pos) {
            return true;
        }
        if matches!(self.lang, "rust" | "python") {
            return false;
        }
        let Some(open) = self.literal_start(pos) else {
            return false;
        };
        if !matches!(self.lang, "ruby" | "php" | "dart" | "elixir") {
            // FD.5b: the compiled and JS languages name libraries in strings
            // too (JS `from 'ioredis'`, Go `import "github.com/go-redis/..."`,
            // C `#include "hiredis.h"`), and connection URLs are strings;
            // only a comment (`//`, `/*`) is refused.
            return !(self.src[open] == b'/'
                && matches!(self.src.get(open + 1), Some(b'/' | b'*')));
        }
        if matches!(self.lang, "ruby" | "php") && self.heredoc_opened_before(open).is_some() {
            return true;
        }
        match self.src[open] {
            b'"' | b'\'' => true,
            // A Dart raw string opens at its `r`.
            b'r' => self.lang == "dart",
            _ => false,
        }
    }

    /// FD.5a: the first content byte of the literal opening at `open` when
    /// that literal interpolates in this file's language, else `None`: a Ruby
    /// / PHP `".."` or an unquoted / `"ID"` heredoc (whose map entry starts
    /// at its body, the line after the opener), a Dart `'..'` / `".."` /
    /// triple string (a raw `r'..'` starts at its `r`), an Elixir `".."` /
    /// `'..'` / triple string. Comments (`#`, `//`, `/*`, `=begin`), Ruby
    /// and PHP `'..'` and Elixir `?c` char literals open none.
    fn interpolation_body(&self, open: usize) -> Option<usize> {
        let at = *self.src.get(open)?;
        if matches!(self.lang, "ruby" | "php")
            && let Some(quoted) = self.heredoc_opened_before(open)
        {
            return (!quoted).then_some(open);
        }
        let opens = match self.lang {
            "ruby" | "php" => at == b'"',
            "dart" | "elixir" => matches!(at, b'"' | b'\''),
            _ => false,
        };
        opens.then_some(open + 1)
    }

    /// When the literal at `open` is a heredoc body (it starts a line and the
    /// line before it holds a Ruby `<<~` / `<<-` or PHP `<<<` opener in
    /// code), whether its identifier is `'`-quoted (Ruby `<<~'ID'`, a PHP
    /// nowdoc `<<<'ID'`: no interpolation); `None` when it is no heredoc.
    fn heredoc_opened_before(&self, open: usize) -> Option<bool> {
        if open == 0 || self.src[open - 1] != b'\n' {
            return None;
        }
        let line_end = open - 1;
        let line_start = self.src[..line_end]
            .iter()
            .rposition(|&c| c == b'\n')
            .map_or(0, |i| i + 1);
        let line = &self.src[line_start..line_end];
        let marks: &[&[u8]] = if self.lang == "ruby" {
            &[b"<<~", b"<<-"]
        } else {
            &[b"<<<"]
        };
        (0..line.len()).rev().find_map(|k| {
            let mark = marks.iter().find(|m| line[k..].starts_with(m))?;
            if self.literal_start(line_start + k).is_some() {
                return None;
            }
            let mut q = k + mark.len();
            while line.get(q).is_some_and(|c| *c == b' ' || *c == b'\t') {
                q += 1;
            }
            Some(line.get(q) == Some(&b'\''))
        })
    }

    /// FD.5a: true when byte `pos`, inside the literal opening at `open`,
    /// sits inside one of its interpolation spans: Ruby / Elixir `#{..}`,
    /// Dart `${..}`, PHP `{$..}` / `${..}`, the span running to its balanced
    /// `}`. Scans the literal from its first content byte to `pos`;
    /// a `\` escapes the next byte outside a span (`"\#{x}"` is text).
    fn in_interpolation(&self, open: usize, pos: usize) -> bool {
        let Some(body) = self.interpolation_body(open) else {
            return false;
        };
        let opener = |i: usize| -> bool {
            let (a, b) = (self.src[i], self.src.get(i + 1).copied());
            match self.lang {
                "ruby" | "elixir" => a == b'#' && b == Some(b'{'),
                "dart" => a == b'$' && b == Some(b'{'),
                "php" => (a == b'{' && b == Some(b'$')) || (a == b'$' && b == Some(b'{')),
                _ => false,
            }
        };
        let mut depth = 0usize;
        let mut i = body;
        while i < pos.min(self.src.len()) {
            match self.src[i] {
                b'\\' if depth == 0 => {
                    i += 2;
                    continue;
                }
                _ if depth == 0 && opener(i) => {
                    depth = 1;
                    i += 2;
                    continue;
                }
                b'{' if depth > 0 => depth += 1,
                b'}' if depth > 0 => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        depth > 0
    }

    /// FD.4b: true when the string literal a scanner's own lexer opened at
    /// `open` (its delimiter; `body` is its first content byte) is a literal
    /// this map also opens there, in code, and the literal is a call
    /// argument: the code before it (blanks, comments and a Rust `&` skipped)
    /// is the call's `(`, a `,` inside it, or (Python) a keyword argument's
    /// `name=`, and the `(` follows a callee ([`CodeGuard::is_callee_paren`]).
    /// Text nested in another literal or a comment fails the first test; a
    /// `const` / `let` / field / tuple value fails the second. FD.5a: the
    /// second test is Rust's and Python's only (the module doc's "Statement
    /// literals"); a Ruby / PHP / Dart / Elixir literal passes on the first,
    /// and so (FD.5b) does a Go / JVM / C-family / Swift / JS one.
    pub(crate) fn opens_argument(&self, open: usize, body: usize) -> bool {
        let Some(lo) = self.same_literal(open, body) else {
            return false;
        };
        if !matches!(self.lang, "rust" | "python") {
            return true;
        }
        // FD.6: a statement literal inside a Rust `#[cfg(test)]` / `#[test]`
        // item is a test's input (the text a scanner or parser under test
        // reads), not the data access of the code it sits in.
        if self.lang == "rust" && self.in_rust_test(lo) {
            return false;
        }
        // Any prefix letters glued to the literal belong to it.
        let mut first = lo;
        while first > 0 && lo - first < 3 && self.src[first - 1].is_ascii_alphabetic() {
            first -= 1;
        }
        let Some(mut at) = self.prev_code(first) else {
            return false;
        };
        if self.lang == "rust" && self.src[at] == b'&' {
            let Some(p) = self.prev_code(at) else {
                return false;
            };
            at = p;
        }
        match self.src[at] {
            b'(' => self.is_callee_paren(at),
            b',' => self
                .enclosing_open(at)
                .is_some_and(|p| self.is_callee_paren(p)),
            b'=' if self.lang == "python" => {
                // `name=` but not `==` / `<=` / `>=` / `!=` / `x = `'s
                // statement form outside a call.
                let Some(k) = self.prev_code(at) else {
                    return false;
                };
                let kwarg = is_ident_byte(self.src[k])
                    && self.src.get(at + 1) != Some(&b'=')
                    && !matches!(
                        self.src.get(at.wrapping_sub(1)),
                        Some(b'=' | b'<' | b'>' | b'!')
                    );
                kwarg
                    && self
                        .enclosing_open(k)
                        .is_some_and(|p| self.is_callee_paren(p))
            }
            _ => false,
        }
    }

    /// The lower of the two literal starts when the literal a scanner's own
    /// lexer opened at `open` (its delimiter; `body` its first content byte)
    /// is the literal this map holds `body` in, opening in code: the map
    /// and the scanner may disagree on a string prefix (`r#`, `f`, `rb`), so
    /// the two starts must differ only by up to three prefix bytes. `None`
    /// when `body` is code, or the map's literal opens elsewhere (the
    /// scanner's delimiter sits inside another literal or a comment).
    fn same_literal(&self, open: usize, body: usize) -> Option<usize> {
        let start = self.literal_start(body)?;
        let (lo, hi) = (start.min(open), start.max(open));
        let prefix_only = self.src[lo..hi]
            .iter()
            .all(|c| c.is_ascii_alphabetic() || *c == b'#');
        (start < body && hi - lo <= 3 && prefix_only).then_some(lo)
    }

    /// FD.6: true when, in a Rust or Python file, the literal a scanner read
    /// with its delimiter at `open` and first content byte at `body` is a
    /// real literal of the file ([`CodeGuard::same_literal`]): a delimiter
    /// the scanner found inside another literal (a backtick template held
    /// in a test's Rust string sample) or a comment opens none. In
    /// every other guarded language this is true.
    pub(crate) fn opens_own_literal(&self, open: usize, body: usize) -> bool {
        !matches!(self.lang, "rust" | "python") || self.same_literal(open, body).is_some()
    }

    /// FD.6: true when byte `pos` of a Rust file sits inside an item
    /// annotated `#[cfg(test)]` or a test attribute (`#[test]`,
    /// `#[tokio::test]`, any attribute path ending in `test`), from the
    /// attribute to the item's closing `}` (or `;` for a bodiless item).
    /// False in every other language.
    pub(crate) fn in_rust_test(&self, pos: usize) -> bool {
        if self.lang != "rust" {
            return false;
        }
        self.test_spans
            .get_or_init(|| self.rust_test_spans())
            .iter()
            .any(|&(s, e)| s <= pos && pos < e)
    }

    /// The spans [`CodeGuard::in_rust_test`] reads, sorted by start.
    fn rust_test_spans(&self) -> Vec<(usize, usize)> {
        let src = self.src;
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while let Some(off) = src[i..].windows(2).position(|w| w == b"#[") {
            let at = i + off;
            i = at + 2;
            if self.literal_start(at).is_some() {
                continue;
            }
            let Some(close) = self.attr_close(at + 2) else {
                continue;
            };
            let attr: Vec<u8> = src[at + 2..close]
                .iter()
                .copied()
                .filter(|c| !c.is_ascii_whitespace())
                .collect();
            let is_test = attr == b"cfg(test)" || attr == b"test" || attr.ends_with(b"::test");
            if !is_test {
                continue;
            }
            if let Some(end) = self.item_end(close + 1) {
                spans.push((at, end));
                i = i.max(close + 1);
            }
        }
        spans
    }

    /// The `]` closing the attribute whose content starts at `from`
    /// (brackets balanced, literals skipped), within [`ARG_WINDOW`] bytes.
    fn attr_close(&self, from: usize) -> Option<usize> {
        let mut depth = 0usize;
        let end = (from + ARG_WINDOW).min(self.src.len());
        let mut i = from;
        while i < end {
            if self.literal_start(i).is_some() {
                i += 1;
                continue;
            }
            match self.src[i] {
                b'[' => depth += 1,
                b']' if depth == 0 => return Some(i),
                b']' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// The end (one past the last byte) of the item starting at or after
    /// `from`: its balanced `{..}` body, or the `;` of a bodiless item
    /// (`mod tests;`), literals and comments skipped.
    fn item_end(&self, from: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut i = from;
        while i < self.src.len() {
            if self.literal_start(i).is_some() {
                i += 1;
                continue;
            }
            match self.src[i] {
                b'{' => depth += 1,
                b'}' if depth > 0 => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                b';' if depth == 0 => return Some(i + 1),
                _ => {}
            }
            i += 1;
        }
        None
    }

    /// The last code byte before `pos` that is not blank, stepping over
    /// comments (and literals) whole; `None` at the file start.
    fn prev_code(&self, pos: usize) -> Option<usize> {
        let mut i = pos;
        while i > 0 {
            i -= 1;
            if let Some(start) = self.literal_start(i) {
                i = start;
                continue;
            }
            if !self.src[i].is_ascii_whitespace() {
                return Some(i);
            }
        }
        None
    }

    /// The unmatched `(` holding byte `pos` (literals and comments skipped,
    /// within [`ARG_WINDOW`] bytes); `None` when the nearest unmatched
    /// bracket is a `[` / `{`, a Rust `;` ends the statement first, or
    /// nothing opens in reach.
    fn enclosing_open(&self, pos: usize) -> Option<usize> {
        let floor = pos.saturating_sub(ARG_WINDOW);
        let mut depth = 0usize;
        let mut i = pos;
        while i > floor {
            i -= 1;
            if let Some(start) = self.literal_start(i) {
                i = start;
                continue;
            }
            match self.src[i] {
                b')' | b']' | b'}' => depth += 1,
                b'(' if depth == 0 => return Some(i),
                b'[' | b'{' if depth == 0 => return None,
                b'(' | b'[' | b'{' => depth -= 1,
                b';' if depth == 0 && self.lang == "rust" => return None,
                _ => {}
            }
        }
        None
    }

    /// True when the `(` at `paren` opens a call's argument list: it follows
    /// a callee name that is no keyword (`return (`, `if (`), a Rust macro's
    /// `!`, a generic / turbofish `>`, or a call / index result (`)`, `]`).
    fn is_callee_paren(&self, paren: usize) -> bool {
        let Some(k) = self.prev_code(paren) else {
            return false;
        };
        match self.src[k] {
            b'!' | b'>' | b')' | b']' => true,
            c if is_ident_byte(c) => {
                let mut s = k;
                while s > 0 && is_ident_byte(self.src[s - 1]) {
                    s -= 1;
                }
                let word = &self.src[s..=k];
                !NOT_CALLEES.iter().any(|kw| kw.as_bytes() == word)
            }
            _ => false,
        }
    }

    /// True when the literal opening at `open` is a Python f-string: its
    /// quote is preceded by a string prefix of one or two letters from
    /// `fFrRbBuU` holding an `f` / `F`, and the prefix by a non-identifier
    /// byte or the file start (so `elif"x"` is no prefix). A comment's `#`
    /// opens no f-string.
    fn is_fstring(&self, open: usize) -> bool {
        if !matches!(self.src.get(open), Some(b'"' | b'\'')) {
            return false;
        }
        let mut start = open;
        let mut has_f = false;
        while open - start < 2
            && start > 0
            && matches!(
                self.src[start - 1],
                b'f' | b'F' | b'r' | b'R' | b'b' | b'B' | b'u' | b'U'
            )
        {
            has_f |= matches!(self.src[start - 1], b'f' | b'F');
            start -= 1;
        }
        has_f && (start == 0 || !is_ident_byte(self.src[start - 1]))
    }
}

/// A [`CodeGuard`] built on first use, plus the tally of refused occurrences
/// behind the `[code-guard]` marker. Most files never reach a candidate needle
/// (a scanner checks its library signals first), so most files never lex.
pub(crate) struct LazyGuard<'a> {
    path: &'a str,
    source: &'a str,
    built: Option<Option<CodeGuard<'a>>>,
    dropped: usize,
}

impl<'a> LazyGuard<'a> {
    /// A guard for `source` read from `path`; lexes nothing yet. An empty
    /// `path` (a caller with no file) is a file with no guard.
    pub(crate) fn new(path: &'a str, source: &'a str) -> Self {
        LazyGuard {
            path,
            source,
            built: None,
            dropped: 0,
        }
    }

    /// True when the occurrence starting at byte `pos` may be a call site:
    /// the file has no guard, or `pos` is code ([`CodeGuard::is_code`]). A
    /// refusal is counted for [`LazyGuard::report`].
    pub(crate) fn admits(&mut self, pos: usize) -> bool {
        self.check(pos, CodeGuard::is_code)
    }

    /// CJ.1c: [`LazyGuard::admits`] for a needle that is itself a quoted key
    /// (`'schedule':`): true when the file has no guard or the literal holding
    /// `pos` opens at `pos` ([`CodeGuard::opens_literal`]). A refusal is
    /// counted for [`LazyGuard::report`].
    pub(crate) fn admits_key(&mut self, pos: usize) -> bool {
        self.check(pos, CodeGuard::opens_literal)
    }

    /// FD.5a: [`LazyGuard::admits`] for a scanner that reads library names
    /// ([`CodeGuard::is_code_or_script_string`]): a comment is refused in
    /// every guarded language, a Ruby / PHP / Dart / Elixir import string is
    /// admitted. A refusal is counted for [`LazyGuard::report`].
    pub(crate) fn admits_name(&mut self, pos: usize) -> bool {
        self.check(pos, CodeGuard::is_code_or_script_string)
    }

    /// FD.4b: true when the file has no guard, or the string literal whose
    /// delimiter opens at `open` (a scanner's own lexer's reading) and whose
    /// body starts at `body` opens in code AND stands as a call argument
    /// ([`CodeGuard::opens_argument`]): a SQL / Cypher statement literal is a
    /// data access only as the argument of the call that runs it. A refusal
    /// is counted for [`LazyGuard::report`].
    pub(crate) fn admits_argument(&mut self, open: usize, body: usize) -> bool {
        self.check(open, |g, open| g.opens_argument(open, body))
    }

    /// FD.6: true when the file has no guard, or the literal a scanner read
    /// with its delimiter at `open` and first content byte at `body` is a
    /// literal of the file itself ([`CodeGuard::opens_own_literal`]). A
    /// refusal is counted for [`LazyGuard::report`].
    pub(crate) fn admits_literal(&mut self, open: usize, body: usize) -> bool {
        self.check(open, |g, open| g.opens_own_literal(open, body))
    }

    /// True when the file has no guard or `rule` accepts `pos`; builds the
    /// guard on first use and counts a refusal.
    fn check(&mut self, pos: usize, rule: impl FnOnce(&CodeGuard<'a>, usize) -> bool) -> bool {
        let (path, source) = (self.path, self.source);
        let guard = self
            .built
            .get_or_insert_with(|| CodeGuard::for_path(path, source));
        match guard {
            Some(g) if !rule(g, pos) => {
                self.dropped += 1;
                false
            }
            _ => true,
        }
    }

    /// The fired_on marker, printed only when an occurrence was refused:
    /// `[code-guard] <scanner> lang=<l> dropped=<n> path=<path>`, `<l>` a
    /// [`CodeGuard::lang`] tag.
    pub(crate) fn report(&self, scanner: &str) {
        if self.dropped == 0 {
            return;
        }
        let lang = self
            .built
            .as_ref()
            .and_then(|g| g.as_ref())
            .map_or("none", |g| g.lang());
        eprintln!(
            "[code-guard] {scanner} lang={lang} dropped={} path={}",
            self.dropped, self.path
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Is the first byte of `needle` (its first occurrence) code under the
    /// guard for `path`?
    fn code_at(path: &str, src: &str, needle: &str) -> bool {
        let at = src.find(needle).expect("needle in source");
        CodeGuard::for_path(path, src).expect("a guard").is_code(at)
    }

    #[test]
    fn rust_literals_and_comments_are_not_code() {
        for src in [
            "let a = \"nc.publish(x)\";\n",
            "let a = r#\"nc.publish(x)\"#;\n",
            "let a = r\"nc.publish(x)\";\n",
            "let a = b\"nc.publish(x)\";\n",
            "let a = br#\"nc.publish(x)\"#;\n",
            "// nc.publish(x)\n",
            "/// nc.publish(x)\nfn f() {}\n",
            "//! nc.publish(x)\n",
            "/* outer /* inner */ nc.publish(x) */\n",
            "let a = \"escaped \\\" quote nc.publish(x)\";\n",
        ] {
            assert!(
                !code_at("src/x.rs", src, "nc.publish("),
                "{src:?} is a literal / comment"
            );
        }
        for src in [
            "nc.publish(\"orders\", data);\n",
            "fn f<'a>(s: &'a str) { nc.publish(s, d); }\n",
            "let q = '\"'; nc.publish(\"orders\", d);\n",
            "let s = \"x\"; // tail\nnc.publish(\"orders\", d);\n",
            "/* a */ nc.publish(\"orders\", d);\n",
        ] {
            assert!(code_at("src/x.rs", src, "nc.publish("), "{src:?} is code");
        }
    }

    #[test]
    fn python_literals_and_comments_are_not_code() {
        for src in [
            "S = 'ch.basic_publish(x)'\n",
            "S = \"ch.basic_publish(x)\"\n",
            "S = r'ch.basic_publish(x)'\n",
            "S = b\"ch.basic_publish(x)\"\n",
            "S = '''\nch.basic_publish(x)\n'''\n",
            "S = \"\"\"\nch.basic_publish(x)\n\"\"\"\n",
            "# ch.basic_publish(x)\n",
            "x = 1  # ch.basic_publish(x)\n",
        ] {
            assert!(
                !code_at("tools/x.py", src, "ch.basic_publish("),
                "{src:?} is a literal / comment"
            );
        }
        assert!(code_at(
            "tools/x.py",
            "S = 'a'\nch.basic_publish(exchange='')\n",
            "ch.basic_publish("
        ));
        assert!(code_at(
            "tools/x.pyw",
            "ch.basic_publish(exchange='')\n",
            "ch.basic_publish("
        ));
        assert!(!code_at(
            "tools/x.pyi",
            "# ch.basic_publish(x)\n",
            "ch.basic_publish("
        ));
        assert!(!code_at(
            "tools/X.PY",
            "# ch.basic_publish(x)\n",
            "ch.basic_publish("
        ));
    }

    #[test]
    fn python_fstrings_are_code() {
        for src in [
            "URL = f\"redis://{os.getenv('REDIS_HOST')}\"\n",
            "URL = F'x {os.getenv(\"REDIS_HOST\")}'\n",
            "URL = rf\"x {os.getenv('REDIS_HOST')}\"\n",
            "URL = Fr'x {os.getenv(\"REDIS_HOST\")}'\n",
            "URL = f\"\"\"\n{os.getenv('REDIS_HOST')}\n\"\"\"\n",
            "f'{os.getenv(\"A\")}'\n",
        ] {
            let g = CodeGuard::for_path("settings.py", src).expect("a guard");
            let open = src.find(['"', '\'']).expect("a quote");
            let at = src.find("os.getenv(").expect("needle");
            assert!(g.is_code(at), "{src:?}: an f-string's field is code");
            assert!(
                g.is_code(open),
                "{src:?}: every byte of an f-string is code"
            );
            assert_eq!(
                g.literal_start(at),
                Some(open),
                "{src:?}: the literal opens at its quote"
            );
        }
        // An identifier then a quote is no prefix; a byte / raw string and a
        // comment stay refused.
        for src in [
            "if x: pass\nelif\"os.getenv('A')\": pass\n",
            "U = b'os.getenv(1)'\n",
            "U = r'os.getenv(1)'\n",
            "U = buf'os.getenv(1)'\n",
            "# f'os.getenv(1)'\n",
        ] {
            assert!(
                !code_at("a.py", src, "os.getenv("),
                "{src:?} is not an f-string"
            );
        }
        let src = "x = 1  # os.getenv('A')\n";
        let g = CodeGuard::for_path("a.py", src).expect("a guard");
        assert_eq!(g.literal_start(src.find("os.").unwrap()), src.find('#'));
        assert_eq!(g.literal_start(0), None);
        // Rust has no f-string rule.
        assert!(!code_at(
            "a.rs",
            "let u = f\"os.getenv(1)\";\n",
            "os.getenv("
        ));
    }

    #[test]
    fn other_files_have_no_guard() {
        for path in [
            "a.vue",
            "a.svelte",
            "a.html",
            "a.erb",
            "",
            "src/rs",
            "a.rs.txt",
            "dir.rs/a.vue",
        ] {
            assert!(
                CodeGuard::for_path(path, "// x.send(\n").is_none(),
                "{path:?}"
            );
        }
        assert_eq!(
            CodeGuard::for_path("A.RS", "").map(|g| g.lang()),
            Some("rust")
        );
        assert_eq!(
            CodeGuard::for_path("a.py", "").map(|g| g.lang()),
            Some("python")
        );
        for (path, lang) in [
            ("app/a.rb", "ruby"),
            ("lib/tasks/x.RAKE", "ruby"),
            ("x.gemspec", "ruby"),
            ("src/a.php", "php"),
            ("lib/a.dart", "dart"),
            ("lib/a.ex", "elixir"),
            ("test/a_test.exs", "elixir"),
            ("pkg/a.go", "go"),
            ("src/A.java", "java"),
            ("src/A.kt", "kotlin"),
            ("build.gradle.kts", "kotlin"),
            ("src/A.scala", "scala"),
            ("w.sc", "scala"),
            ("src/A.cs", "csharp"),
            ("Sources/A.swift", "swift"),
            ("src/a.c", "c"),
            ("include/a.h", "c"),
            ("src/a.cc", "cpp"),
            ("src/a.CPP", "cpp"),
            ("src/a.cxx", "cpp"),
            ("include/a.hpp", "cpp"),
            ("include/a.hh", "cpp"),
            ("src/a.js", "javascript"),
            ("src/a.jsx", "javascript"),
            ("src/a.mjs", "javascript"),
            ("src/a.cjs", "javascript"),
            ("src/a.ts", "typescript"),
            ("src/a.tsx", "typescript"),
            ("src/a.mts", "typescript"),
            ("src/a.cts", "typescript"),
        ] {
            assert_eq!(
                CodeGuard::for_path(path, "").map(|g| g.lang()),
                Some(lang),
                "{path:?}"
            );
        }
        let src = "// producer.send(\"x\")\n";
        let mut guard = LazyGuard::new("src/a.vue", src);
        assert!(guard.admits(3));
        assert_eq!(guard.dropped, 0);
        let mut none = LazyGuard::new("", src);
        assert!(none.admits(3));
        assert_eq!(none.dropped, 0);
    }

    #[test]
    fn quoted_key_must_open_its_literal() {
        // Python: a dict key opens its own literal; the same key inside a
        // larger string or a comment does not.
        let py = "S = {'schedule': 1}\nT = \"{'schedule': 2}\"\n# 'schedule': 3\n";
        let at: Vec<usize> = py.match_indices("'schedule':").map(|(i, _)| i).collect();
        let g = CodeGuard::for_path("x.py", py).expect("a guard");
        assert_eq!(
            at.iter().map(|&i| g.opens_literal(i)).collect::<Vec<_>>(),
            [true, false, false]
        );
        // Rust: `"schedule":` opens a literal; a `'schedule':` lexes as code
        // (no char literal), and one inside a string or comment is held by it.
        let rs = "let a = json!({\"schedule\": 1});\nlet b = 'schedule':;\nlet c = \"'schedule':\";\n// \"schedule\":\n";
        let g = CodeGuard::for_path("x.rs", rs).expect("a guard");
        let dq: Vec<usize> = rs.match_indices("\"schedule\":").map(|(i, _)| i).collect();
        let sq: Vec<usize> = rs.match_indices("'schedule':").map(|(i, _)| i).collect();
        assert!(g.opens_literal(dq[0]));
        assert!(!g.opens_literal(dq[1]), "inside a // comment");
        assert!(
            !g.opens_literal(sq[0]),
            "a bare 'schedule': is code in Rust"
        );
        assert!(!g.opens_literal(sq[1]), "inside a string");
        // No guard admits every key; a refusal is counted.
        let mut none = LazyGuard::new("app.vue", "x = \"{'schedule': 1}\"");
        assert!(none.admits_key(6));
        assert_eq!(none.dropped, 0);
        let mut guard = LazyGuard::new("x.py", py);
        assert!(guard.admits_key(at[0]));
        assert!(!guard.admits_key(at[1]));
        assert!(!guard.admits_key(at[2]));
        assert_eq!(guard.dropped, 2);
    }

    #[test]
    fn lazy_guard_counts_refusals() {
        let src = "// producer.send(\"x\")\nproducer.send(\"orders\", m);\n";
        let mut guard = LazyGuard::new("src/a.rs", src);
        assert!(
            guard.built.is_none(),
            "nothing is lexed before the first admit"
        );
        let in_comment = src.find("producer").unwrap();
        let in_code = src.rfind("producer").unwrap();
        assert!(!guard.admits(in_comment));
        assert!(guard.admits(in_code));
        assert!(!guard.admits(in_comment + 1));
        assert_eq!(guard.dropped, 2);
        assert!(matches!(guard.built, Some(Some(_))));
    }

    /// FD.4b: the first literal of `needle`'s occurrence in `src`, read the
    /// way the data-entity lexer reports it (`open` the delimiter, `body`
    /// the next byte).
    fn argument_at(path: &str, src: &str, needle: &str) -> bool {
        let open = src.find(needle).unwrap() + needle.find('"').unwrap();
        let body = open + 1;
        let mut guard = LazyGuard::new(path, src);
        guard.admits_argument(open, body)
    }

    /// FD.6: the spans of a Rust file's test items, from the attribute to
    /// the item's closing `}` / `;`; braces inside literals and comments do
    /// not end one, and a `#[test]` in a string opens none.
    #[test]
    fn rust_test_items_are_spanned() {
        let src = "fn prod() { let a = \"#[test]\"; }\n\
                   #[cfg(test)]\nmod tests {\n    fn x() { let s = \"}\"; /* } */ }\n    // }\n    fn y() {}\n}\n\
                   fn after() {}\n\
                   #[tokio::test]\nasync fn t() { inner(); }\n\
                   #[cfg(test)]\nmod more;\n\
                   fn last() {}\n";
        let g = CodeGuard::for_path("src/a.rs", src).expect("a guard");
        let at = |needle: &str| src.find(needle).expect("needle in source");
        assert!(!g.in_rust_test(at("prod")));
        assert!(!g.in_rust_test(at("let a")));
        assert!(g.in_rust_test(at("mod tests")));
        assert!(g.in_rust_test(at("fn y")));
        assert!(!g.in_rust_test(at("fn after")));
        assert!(g.in_rust_test(at("inner")));
        assert!(g.in_rust_test(at("mod more")));
        assert!(!g.in_rust_test(at("fn last")));
        // Only Rust has test items.
        let py = CodeGuard::for_path("a.py", "#[test]\ndef f():\n    pass\n").expect("a guard");
        assert!(!py.in_rust_test(10));
        // A statement literal in a test item is no call argument.
        let rs =
            "#[test]\nfn t() { db.query(\"SELECT 1\"); }\nfn p() { db.query(\"SELECT 2\"); }\n";
        assert!(!argument_at("src/a.rs", rs, "\"SELECT 1"));
        assert!(argument_at("src/a.rs", rs, "\"SELECT 2"));
    }

    /// FD.6: a literal a scanner reads must be one the file holds itself in
    /// Rust and Python: a backtick or quote inside another string is none.
    #[test]
    fn own_literals_open_in_code() {
        let own = |path: &str, src: &str, needle: &str| {
            let open = src.find(needle).expect("needle in source");
            LazyGuard::new(path, src).admits_literal(open, open + 1)
        };
        let rs =
            "let a = gql(\"type Query { a: Int }\");\nlet b = \"gql`type Query { b: Int }`\";\n";
        assert!(own("src/a.rs", rs, "\"type Query { a"));
        assert!(!own("src/a.rs", rs, "`type Query { b"));
        let py = "a = gql(\"type Query { a: Int }\")\nb = \"gql`type Query { b: Int }`\"\n";
        assert!(own("a.py", py, "\"type Query { a"));
        assert!(!own("a.py", py, "`type Query { b"));
        // Other languages and unguarded files keep every literal.
        assert!(own(
            "a.ts",
            "const b = gql`type Query { b: Int }`;\n",
            "`type"
        ));
        assert!(own("", rs, "`type Query { b"));
    }

    #[test]
    fn statement_literal_must_open_as_a_call_argument() {
        for (src, needle) in [
            ("db.query(\"SELECT 1\");\n", "\"SELECT"),
            ("q::<_, T>(\"SELECT 1\");\n", "\"SELECT"),
            ("f(&format!(\"SELECT 1\"));\n", "\"SELECT"),
            ("f(a,\n  // c\n  \"SELECT 1\");\n", "\"SELECT"),
            ("f(a, g(b), \"SELECT 1\");\n", "\"SELECT"),
        ] {
            assert!(
                argument_at("src/a.rs", src, needle),
                "{src:?} is an argument"
            );
        }
        for (src, needle) in [
            ("let q = \"SELECT 1\";\n", "\"SELECT"),
            ("const Q: &str = \"SELECT 1\";\n", "\"SELECT"),
            ("let t = (\"a\", \"SELECT 1\");\n", "\"SELECT"),
            ("let v = [f(x), \"SELECT 1\"];\n", "\"SELECT"),
            ("if (\"SELECT 1\" == x) {}\n", "\"SELECT"),
            ("let s = \"f(\\\"SELECT 1\\\")\";\n", "\\\"SELECT"),
            ("// f(\"SELECT 1\")\n", "\"SELECT"),
        ] {
            assert!(
                !argument_at("src/a.rs", src, needle),
                "{src:?} is no argument"
            );
        }
        assert!(argument_at(
            "a.py",
            "cur.execute(sql=\"SELECT 1\")\n",
            "\"SELECT"
        ));
        assert!(!argument_at("a.py", "Q = \"SELECT 1\"\n", "\"SELECT"));
        assert!(!argument_at("a.py", "f(a == \"SELECT 1\")\n", "\"SELECT"));
        assert!(!argument_at("a.py", "return (\"SELECT 1\")\n", "\"SELECT"));
        // A file without a lexer admits everything.
        assert!(argument_at("a.vue", "q := \"SELECT 1\"\n", "\"SELECT"));
        // FD.5b: the compiled / JS languages skip the argument rule (a
        // statement opening in code passes), a commented one is refused.
        assert!(argument_at("a.go", "q := \"SELECT 1\"\n", "\"SELECT"));
        assert!(argument_at("a.ts", "const q = \"SELECT 1\";\n", "\"SELECT"));
        assert!(argument_at(
            "A.java",
            "String q = \"SELECT 1\";\n",
            "\"SELECT"
        ));
        assert!(!argument_at(
            "a.go",
            "// db.Query(\"SELECT 1\")\n",
            "\"SELECT"
        ));
        assert!(!argument_at(
            "a.kt",
            "/* db.query(\"SELECT 1\") */\n",
            "\"SELECT"
        ));
    }

    /// FD.5a: per language, `refused` sources hold the needle in a comment
    /// or a plain / non-interpolating literal; `admitted` ones in code or in
    /// an interpolation span.
    fn assert_script(path: &str, needle: &str, refused: &[&str], admitted: &[&str]) {
        for src in refused {
            assert!(!code_at(path, src, needle), "{path}: {src:?} is text");
        }
        for src in admitted {
            assert!(code_at(path, src, needle), "{path}: {src:?} is code");
        }
    }

    #[test]
    fn ruby_comments_strings_and_interpolation() {
        assert_script(
            "app/consumers/karafka.rb",
            "topic ",
            &[
                // The Karafka seed case (§E W2): a commented route.
                "require 'karafka'\n# topic \"ghost_rb\"\nclass App; end\n",
                "x = 1 # topic :ghost\n",
                "S = 'topic :ghost'\n",
                "S = \"topic :ghost\"\n",
                "S = \"orders #{a} topic :ghost\"\n",
                "S = \"\\#{topic :ghost}\"\n",
                "=begin\ntopic :ghost\n=end\n",
                "S = <<~SQL\n  topic :ghost\nSQL\n",
                "S = <<~'SQL'\n  #{topic :ghost}\nSQL\n",
            ],
            &[
                "routes.draw { topic :orders do end }\n",
                "S = \"x\"; topic :orders\n",
                "S = \"orders.#{topic :x}\"\n",
                "S = \"a #{b} c #{ {k: 1}; topic :x}\"\n",
                "S = <<~SQL\n  #{topic :x}\nSQL\n",
            ],
        );
    }

    #[test]
    fn php_comments_strings_and_interpolation() {
        assert_script(
            "src/pub.php",
            "basic_publish(",
            &[
                "<?php // $channel->basic_publish($m, '', 'ghost_php');\n",
                "<?php # basic_publish(1);\n",
                "<?php /* basic_publish(1); */\n",
                "<?php $s = 'basic_publish(1)';\n",
                "<?php $s = \"x basic_publish(1)\";\n",
                "<?php $s = \"$ch basic_publish(1)\";\n",
                "<?php $s = \"{$a} basic_publish(1)\";\n",
                "<?php $s = <<<'EOT'\n{$ch->basic_publish(1)}\nEOT;\n",
                "<?php $s = <<<EOT\nbasic_publish(1)\nEOT;\n",
            ],
            &[
                "<?php $channel->basic_publish($msg, '', 'invoices');\n",
                "<?php $s = \"x {$ch->basic_publish(1)} y\";\n",
                "<?php $s = \"x ${basic_publish(1)}\";\n",
                "<?php $s = <<<EOT\n{$ch->basic_publish(1)}\nEOT;\n",
                "<?php #[Attr] $ch->basic_publish(1);\n",
            ],
        );
    }

    #[test]
    fn dart_comments_strings_and_interpolation() {
        assert_script(
            "lib/bus.dart",
            "bus.fire(",
            &[
                "// bus.fire(GhostEvent())\n",
                "/// bus.fire(GhostEvent())\n",
                "/* a /* b */ bus.fire(GhostEvent()) */\n",
                "final s = 'bus.fire(x)';\n",
                "final s = \"bus.fire(x)\";\n",
                "final s = '''\nbus.fire(x)\n''';\n",
                "final s = r'${bus.fire(x)}';\n",
                "final s = '$bus bus.fire(x)';\n",
            ],
            &[
                "void main() { bus.fire(OrderPlaced()); }\n",
                "final s = 'x ${bus.fire(x)}';\n",
                "final s = \"\"\"\n${bus.fire(x)}\n\"\"\";\n",
                "final t = #sym; bus.fire(x);\n",
            ],
        );
    }

    #[test]
    fn elixir_comments_strings_and_interpolation() {
        assert_script(
            "lib/pub.ex",
            "Phoenix.PubSub.broadcast(",
            &[
                "# Phoenix.PubSub.broadcast(P, \"ghost_ex\", m)\n",
                "s = \"Phoenix.PubSub.broadcast(P)\"\n",
                "s = 'Phoenix.PubSub.broadcast(P)'\n",
                "@doc \"\"\"\nPhoenix.PubSub.broadcast(P)\n\"\"\"\n",
                "s = \"#{a} Phoenix.PubSub.broadcast(P)\"\n",
            ],
            &[
                "Phoenix.PubSub.broadcast(MyApp.PubSub, \"rooms\", msg)\n",
                "s = \"x #{Phoenix.PubSub.broadcast(P, t, m)}\"\n",
                "s = ~s(Phoenix.PubSub.broadcast(P))\n",
                "x = valid?(a); Phoenix.PubSub.broadcast(P, t, m)\n",
            ],
        );
    }

    #[test]
    fn script_statements_skip_the_argument_rule() {
        // FD.5a: a Ruby / PHP / Dart / Elixir statement literal passes when it
        // opens in code, call argument or not; a commented one is refused.
        assert!(argument_at("a.rb", "SQL = \"SELECT 1\"\n", "\"SELECT"));
        assert!(argument_at(
            "a.php",
            "<?php $q = \"SELECT 1\";\n",
            "\"SELECT"
        ));
        assert!(argument_at(
            "a.dart",
            "db.rawQuery(\"SELECT 1\");\n",
            "\"SELECT"
        ));
        assert!(!argument_at(
            "a.rb",
            "# db.exec(\"SELECT 1\")\n",
            "\"SELECT"
        ));
        assert!(!argument_at(
            "a.ex",
            "# Repo.query(\"SELECT 1\")\n",
            "\"SELECT"
        ));
        assert!(!argument_at(
            "a.php",
            "<?php // $db->query(\"SELECT 1\");\n",
            "\"SELECT"
        ));
    }

    #[test]
    fn library_names_in_script_import_strings_are_admitted() {
        for (path, src, needle) in [
            (
                "lib/a.dart",
                "import 'package:redis/redis.dart';\n",
                "redis.",
            ),
            (
                "lib/a.dart",
                "import r'package:redis/redis.dart';\n",
                "redis.",
            ),
            ("app/a.rb", "require \"redis\"\nRedis.new\n", "redis"),
            ("src/a.php", "<?php require_once 'redis.php';\n", "redis."),
            ("lib/a.ex", "x = \"Redis\"\n", "Redis"),
            ("app/a.rb", "S = <<~X\n  redis.get\nX\n", "redis."),
            ("app/a.rb", "Redis.new\n", "Redis"),
        ] {
            let at = src.find(needle).unwrap();
            let mut guard = LazyGuard::new(path, src);
            assert!(guard.admits_name(at), "{path}: {src:?}");
        }
        for (path, src, needle) in [
            (
                "lib/a.dart",
                "// import 'package:redis/redis.dart';\n",
                "redis.",
            ),
            ("app/a.rb", "# Redis.new\n", "Redis"),
            ("lib/a.ex", "# Redis\n", "Redis"),
            ("src/a.php", "<?php /* redis.get */\n", "redis."),
            ("a.py", "S = 'redis.get'\n", "redis."),
            ("a.rs", "let s = \"redis.get\";\n", "redis."),
        ] {
            let at = src.find(needle).unwrap();
            let mut guard = LazyGuard::new(path, src);
            assert!(!guard.admits_name(at), "{path}: {src:?}");
            assert_eq!(guard.dropped, 1);
        }
    }

    #[test]
    fn js_templates_strings_and_comments() {
        for path in ["src/pub.ts", "src/pub.js", "src/pub.tsx"] {
            assert_script(
                path,
                "nc.publish(",
                &[
                    "// nc.publish('ghost', m)\n",
                    "/* nc.publish('ghost', m) */\n",
                    "/** a\n * nc.publish('ghost', m)\n */\n",
                    "const t = 'nc.publish(x)';\n",
                    "const t = \"nc.publish(x)\";\n",
                    "const t = `nc.publish('ghost.ts', m)`;\n",
                    "const t = `\nline\nnc.publish(x)\n`;\n",
                    "const t = `${a} nc.publish(x)`;\n",
                    "const t = `${`inner`} nc.publish(x)`;\n",
                    "const t = `${f(`${g} nc.publish(x)`)}`;\n",
                    "const t = `\\${nc.publish(x)}`;\n",
                    "const t = 'it\\'s nc.publish(x)';\n",
                ],
                &[
                    "nc.publish('orders', m);\n",
                    "const t = `x`; nc.publish('orders', m);\n",
                    "const t = `a ${nc.publish('x', m)} b`;\n",
                    "const t = `a ${`b ${nc.publish('x', m)}`}`;\n",
                    "const t = `${ {k: 1}.k } ${nc.publish('x', m)}`;\n",
                    "const t = `${'}'} ${nc.publish('x', m)}`;\n",
                    "const s = \"unterminated\nnc.publish('orders', m);\n",
                    "const t = `${a}`; // c\nnc.publish('orders', m);\n",
                    "const u = `${base}/x`; nc.publish(u, m);\n",
                ],
            );
        }
        // The text after a nested template's span is text again, and a
        // template's text piece after a span starts at the span's `}`.
        let src = "const t = `a ${b} c nc.publish(x)`;\n";
        let g = CodeGuard::for_path("a.ts", src).expect("a guard");
        let at = src.find("nc.").unwrap();
        assert_eq!(g.literal_start(at), src.find("} c"));
        assert_eq!(g.literal_start(src.find("b}").unwrap()), None);
    }

    #[test]
    fn go_raw_strings_runes_and_comments() {
        assert_script(
            "pkg/pub.go",
            "nc.Publish(",
            &[
                "// js.Publish(x)\n// nc.Publish(\"ghost.go\", b)\n",
                "/* nc.Publish(\"x\", b) */\n",
                "const doc = `\nnc.Publish(\"ghost.raw\", b)\n`\n",
                "s := \"nc.Publish(x)\"\n",
                "s := \"a \\\" nc.Publish(x)\"\n",
                "s := `${x} nc.Publish(x)`\n",
            ],
            &[
                "func f() { nc.Publish(\"orders.go\", b) }\n",
                // A raw string has no escapes: `a\` closes at its second backtick.
                "const doc = `a\\`; nc.Publish(\"x\", b)\n",
                "r := '\"'; nc.Publish(\"orders\", b)\n",
                "r := '\\''; nc.Publish(\"orders\", b)\n",
                "t := `json:\"x\"`; nc.Publish(\"orders\", b)\n",
                "s := \"x\" // c\nnc.Publish(\"orders\", b)\n",
            ],
        );
    }

    #[test]
    fn kotlin_strings_templates_and_comments() {
        assert_script(
            "src/Pub.kt",
            "kafkaTemplate.send(",
            &[
                "// kafkaTemplate.send(\"ghost.kt\", v)\n",
                "/* a /* b */ kafkaTemplate.send(\"x\", v) */\n",
                "val s = \"kafkaTemplate.send(x)\"\n",
                "val s = \"\"\"\nkafkaTemplate.send(x)\n\"\"\"\n",
                "val s = \"${a} kafkaTemplate.send(x)\"\n",
                "val s = \"$kafkaTemplate.send(x)\"\n",
                "val s = \"\"\"a\"\"\"\" + \"kafkaTemplate.send(x)\"\n",
            ],
            &[
                "fun s() { kafkaTemplate.send(\"orders.kt\", v) }\n",
                "val s = \"x ${kafkaTemplate.send(\"t\", v)}\"\n",
                "val s = \"\"\"\n${kafkaTemplate.send(\"t\", v)}\n\"\"\"\n",
                "val s = \"${\"${kafkaTemplate.send(\"t\", v)}\"}\"\n",
                "val c = '\"'; kafkaTemplate.send(\"orders\", v)\n",
                "val s = \"\"\"a\"\"\"\"; kafkaTemplate.send(\"orders\", v)\n",
                "fun `a test`() { kafkaTemplate.send(\"orders\", v) }\n",
            ],
        );
    }

    #[test]
    fn scala_interpolators_and_plain_strings() {
        assert_script(
            "src/Pub.scala",
            "producer.send(",
            &[
                "// producer.send(record)\n",
                "/* a /* b */ producer.send(r) */\n",
                "val s = \"producer.send(r)\"\n",
                "val s = \"${producer.send(r)}\"\n",
                "val s = \"\"\"\n${producer.send(r)}\n\"\"\"\n",
                "val s = s\"$x producer.send(r)\"\n",
                "val s = s\"$${producer.send(r)}\"\n",
                "val s = f\"$x%d producer.send(r)\"\n",
            ],
            &[
                "def p() = producer.send(new ProducerRecord(\"orders\", v))\n",
                "val s = s\"a ${producer.send(r)}\"\n",
                "val s = s\"\"\"\n${producer.send(r)}\n\"\"\"\n",
                "val s = sql\"x ${producer.send(r)}\"\n",
                "val q = 'sym; producer.send(r)\n",
            ],
        );
    }

    #[test]
    fn swift_raw_strings_and_interpolation() {
        assert_script(
            "Sources/Pub.swift",
            "socket.emit(",
            &[
                "// socket.emit(\"ghost\")\n",
                "/* a /* b */ socket.emit(\"x\") */\n",
                "let s = \"socket.emit(x)\"\n",
                "let s = \"\"\"\nsocket.emit(x)\n\"\"\"\n",
                "let s = #\"socket.emit(x)\"#\n",
                "let s = #\"a \" socket.emit(x)\"#\n",
                "let s = ##\"a \"# socket.emit(x)\"##\n",
                "let s = #\"\\(socket.emit(x))\"#\n",
                "let s = \"\\(a) socket.emit(x)\"\n",
            ],
            &[
                "func f() { socket.emit(\"orders\") }\n",
                "let s = \"a \\(socket.emit(\"x\")) b\"\n",
                "let s = \"a \\(f(g(1), socket.emit(\"x\")))\"\n",
                "let s = #\"a \\#(socket.emit(\"x\"))\"#\n",
                "let s = \"\"\"\n\\(socket.emit(\"x\"))\n\"\"\"\n",
                "#if DEBUG\nsocket.emit(\"orders\")\n#endif\n",
                "let s = \"\\(\"\\(socket.emit(\"x\"))\")\"\n",
            ],
        );
    }

    #[test]
    fn c_family_keeps_code_map_new() {
        for (path, needle, refused, admitted) in [
            (
                "src/P.java",
                "kafkaTemplate.send(",
                "String s = \"\"\"\n  kafkaTemplate.send(x)\n  \"\"\";\n",
                "void p() { kafkaTemplate.send(\"orders\", v); }\n",
            ),
            (
                "src/P.cs",
                "producer.ProduceAsync(",
                "var s = @\"a \"\" producer.ProduceAsync(x)\";\n",
                "await producer.ProduceAsync(\"orders\", m);\n",
            ),
            (
                "src/p.c",
                "redisCommand(",
                "/* redisCommand(c, \"PUBLISH ghost x\") */\n",
                "char q = '\"'; redisCommand(c, \"PUBLISH orders x\");\n",
            ),
            (
                "src/p.cpp",
                "redisCommand(",
                "// redisCommand(c, \"PUBLISH ghost x\")\n",
                "int n = 1'000; redisCommand(c, \"PUBLISH orders x\");\n",
            ),
        ] {
            assert!(!code_at(path, refused, needle), "{path}: {refused:?}");
            assert!(code_at(path, admitted, needle), "{path}: {admitted:?}");
        }
    }

    #[test]
    fn compiled_maps_are_ordered_and_never_panic() {
        let samples = [
            "const t = `a ${f(`b ${'}'} c`)} d`; // x\n/* y */ nc.publish('é', m);\n",
            "s := `raw\n` + \"q\\\"\" + 'é' // c\n/* d */\n",
            "val s = \"\"\"a ${b(\"${c}\")} \"\"\"\" + 'x' /* /* */ */\n",
            "val s = s\"$$ ${a} $b\" + \"\"\"x\"\"\"\n",
            "let s = ##\"a \\##(b(\"\\(c)\")) \"## + \"\\(d)\" /* /* */ */\n",
        ];
        for (path, src) in ["a.ts", "a.go", "a.kt", "a.scala", "a.swift"]
            .iter()
            .zip(samples)
        {
            for cut in 0..=src.len() {
                let Some(prefix) = src.get(..cut) else {
                    continue;
                };
                let g = CodeGuard::for_path(path, prefix).expect("a guard");
                let skips = g.map.skips();
                assert!(
                    skips.windows(2).all(|w| w[0].1 <= w[1].0),
                    "{path} {prefix:?}: {skips:?}"
                );
                assert!(skips.iter().all(|&(s, e)| s < e && e <= prefix.len()));
                for pos in 0..prefix.len() {
                    let _ = g.is_code(pos);
                    let _ = g.is_code_or_script_string(pos);
                }
            }
        }
        // Deep nesting stops following spans, never recursing without bound.
        let deep = "`${".repeat(200) + "nc.publish(x)" + &"}`".repeat(200);
        assert!(CodeGuard::for_path("a.js", &deep).is_some());
    }

    #[test]
    fn compiled_import_strings_are_admitted_comments_refused() {
        for (path, src, needle) in [
            ("src/a.ts", "import Redis from 'ioredis';\n", "redis"),
            (
                "pkg/a.go",
                "import \"github.com/go-redis/redis/v9\"\n",
                "redis",
            ),
            ("src/a.c", "#include \"hiredis.h\"\n", "redis"),
            ("src/A.kt", "val u = \"redis://localhost\"\n", "redis"),
            ("src/a.js", "const u = `redis://${host}`;\n", "redis"),
        ] {
            let at = src.find(needle).unwrap();
            let mut guard = LazyGuard::new(path, src);
            assert!(guard.admits_name(at), "{path}: {src:?}");
        }
        for (path, src, needle) in [
            ("src/a.ts", "// import Redis from 'ioredis';\n", "redis"),
            ("pkg/a.go", "/* redis.NewClient */\n", "redis"),
            ("src/A.swift", "/// redis.get\n", "redis"),
        ] {
            let at = src.find(needle).unwrap();
            let mut guard = LazyGuard::new(path, src);
            assert!(!guard.admits_name(at), "{path}: {src:?}");
            assert_eq!(guard.dropped, 1);
        }
    }
}
