//! CJ.1a: the per-file literal / comment guard every needle scanner shares.
//!
//! A needle scanner reads a call site off raw text, so a needle that sits in a
//! string literal or a comment (a scanner's own needle table, a doc comment, a
//! test's embedded sample source) used to mint a node as if it were a call.
//! In a Rust or Python file this guard refuses an occurrence whose FIRST byte
//! is inside a literal or comment, lexed by LA.20a's [`CodeMap`] (the CLI
//! extractor's rule: "a needle in a doc comment or a string is not a
//! declaration"). Only the needle's first byte is checked: every queue and
//! event needle starts in code at a real site, while the topic literal it
//! reads sits after it.
//!
//! Scope: Rust (`.rs`) and Python (`.py`, `.pyw`, `.pyi`), the languages glia
//! and its tests are written in. Every other file has no guard, so its scans
//! are unchanged.
//!
//! A Python f-string is read as code whole: its `{..}` replacement fields are
//! expressions (`f"redis://{os.getenv('REDIS_HOST')}"`), so an f-string keeps
//! the behaviour it had before the guard, while a plain, raw, byte or
//! triple-quoted string and every comment are refused. Rust needs no such
//! rule: `format!` arguments sit outside the literal and an inline `{x}` holds
//! an identifier only.
//!
//! fired_on marker, once per scanner call that refused an occurrence:
//!   `... 2>&1 | grep '\[code-guard\]'`
//!   `[code-guard] <scanner> lang=<rust|python> dropped=<n> path=<path>`

use crate::cli::{CodeMap, Script};

/// The lexer a file's path selects, by its LAST extension (ASCII
/// case-insensitive): the language tag the marker prints and the
/// [`CodeMap::script`] syntax (`None` = [`CodeMap::new`], the C family's
/// rules, which cover Rust's). `None` for every other file and an empty path.
fn lexer_for(path: &str) -> Option<(&'static str, Option<Script>)> {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (_, ext) = name.rsplit_once('.')?;
    if ext.eq_ignore_ascii_case("rs") {
        Some(("rust", None))
    } else if ["py", "pyw", "pyi"]
        .iter()
        .any(|e| ext.eq_ignore_ascii_case(e))
    {
        Some(("python", Some(Script::Python)))
    } else {
        None
    }
}

fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// One file's literal / comment map, for the languages [`lexer_for`] names.
pub(crate) struct CodeGuard<'a> {
    map: CodeMap,
    src: &'a [u8],
    lang: &'static str,
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
        })
    }

    /// The first byte of the literal or comment holding `pos` (its opening
    /// quote, its `//` / `#`), or `None` when `pos` is code.
    pub(crate) fn literal_start(&self, pos: usize) -> Option<usize> {
        self.map.literal_start(pos)
    }

    /// `rust` or `python`.
    pub(crate) fn lang(&self) -> &'static str {
        self.lang
    }

    /// True when byte `pos` may start a call site: it is code, or (Python
    /// only) it sits in an f-string, whose replacement fields are code.
    pub(crate) fn is_code(&self, pos: usize) -> bool {
        match self.literal_start(pos) {
            None => true,
            Some(open) => self.lang == "python" && self.is_fstring(open),
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
        let (path, source) = (self.path, self.source);
        let guard = self
            .built
            .get_or_insert_with(|| CodeGuard::for_path(path, source));
        match guard {
            Some(g) if !g.is_code(pos) => {
                self.dropped += 1;
                false
            }
            _ => true,
        }
    }

    /// The fired_on marker, printed only when an occurrence was refused:
    /// `[code-guard] <scanner> lang=<rust|python> dropped=<n> path=<path>`.
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
            "a.ts",
            "a.go",
            "a.rb",
            "",
            "src/rs",
            "a.rs.txt",
            "dir.rs/a.go",
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
        let src = "// producer.send(\"x\")\n";
        let mut guard = LazyGuard::new("src/a.ts", src);
        assert!(guard.admits(3));
        assert_eq!(guard.dropped, 0);
        let mut none = LazyGuard::new("", src);
        assert!(none.admits(3));
        assert_eq!(none.dropped, 0);
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
}
