//! Markdown → Confluence **storage-format** (XHTML) — the inverse of
//! [`crate::confluence::storage_to_markdown`].
//!
//! `glia docs push` used to demand a hand-written storage-format body, because
//! nothing in the repo produced one. This module closes the write direction so
//! a plain `.md` file can be pushed (`glia docs push --markdown`).
//!
//! It is deliberately a **subset of CommonMark** — ATX headings, fenced code,
//! flat lists, paragraphs, and inline code / links / strong / em. No tables, no
//! reference links, no nested lists, no setext headings, no HTML passthrough.
//! Unsupported syntax degrades to escaped literal text inside a `<p>`: lossy,
//! but never invalid XHTML, which is the right failure mode for a page body.
//!
//! The oracle is the existing reader: `storage_to_markdown(markdown_to_storage(x))`
//! must preserve the linker-critical structure (code spans, headings, fenced
//! languages, links), and the round-trip tests below mirror
//! `confluence.rs`'s tests one-for-one so the pair stays pinned together.
//!
//! Pure and total — no network, no `unwrap()`, any input produces some output.

/// What the conversion found, for the `[docs] push markdown→storage:` marker.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct StorageStats {
    pub headings: usize,
    pub code_blocks: usize,
    pub links: usize,
    pub inline_code: usize,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ListKind {
    Ul,
    Ol,
}

impl ListKind {
    fn open(self) -> &'static str {
        match self {
            ListKind::Ul => "<ul>",
            ListKind::Ol => "<ol>",
        }
    }
    fn close(self) -> &'static str {
        match self {
            ListKind::Ul => "</ul>",
            ListKind::Ol => "</ol>",
        }
    }
}

/// Convert markdown to Confluence storage-format XHTML.
pub fn markdown_to_storage(md: &str) -> String {
    markdown_to_storage_with_stats(md).0
}

/// As [`markdown_to_storage`], plus a count of what was converted.
pub fn markdown_to_storage_with_stats(md: &str) -> (String, StorageStats) {
    let mut out = String::with_capacity(md.len() + md.len() / 2);
    let mut st = StorageStats::default();

    let mut fence: Option<String> = None;
    let mut fence_body: Vec<&str> = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let mut list: Option<ListKind> = None;

    for line in md.lines() {
        let trimmed = line.trim_start();

        // --- inside a fenced block: everything is verbatim until the closer ---
        if fence.is_some() {
            if trimmed.starts_with("```") {
                let lang = fence.take().unwrap_or_default();
                emit_code(&mut out, &lang, &fence_body);
                st.code_blocks += 1;
                fence_body.clear();
            } else {
                fence_body.push(line);
            }
            continue;
        }

        // --- fence opener ---
        if let Some(info) = trimmed.strip_prefix("```") {
            flush_para(&mut out, &mut para, &mut st);
            close_list(&mut out, &mut list);
            fence = Some(info.trim().to_string());
            fence_body.clear();
            continue;
        }

        // --- ATX heading ---
        if let Some((level, text)) = heading(trimmed) {
            flush_para(&mut out, &mut para, &mut st);
            close_list(&mut out, &mut list);
            out.push_str("<h");
            out.push_str(&level.to_string());
            out.push('>');
            out.push_str(&inline(text, &mut st));
            out.push_str("</h");
            out.push_str(&level.to_string());
            out.push_str(">\n");
            st.headings += 1;
            continue;
        }

        // --- list item (consecutive same-kind items share one list element) ---
        if let Some((kind, item)) = list_item(trimmed) {
            flush_para(&mut out, &mut para, &mut st);
            if list != Some(kind) {
                close_list(&mut out, &mut list);
                out.push_str(kind.open());
                list = Some(kind);
            }
            out.push_str("<li>");
            out.push_str(&inline(item, &mut st));
            out.push_str("</li>");
            continue;
        }

        // --- blank line: close the open blocks ---
        if trimmed.is_empty() {
            flush_para(&mut out, &mut para, &mut st);
            close_list(&mut out, &mut list);
            continue;
        }

        // --- paragraph text (a different block, so it closes any open list) ---
        close_list(&mut out, &mut list);
        para.push(trimmed);
    }

    // EOF: an unterminated fence is still emitted — lossy input must not lose text.
    if let Some(lang) = fence.take() {
        emit_code(&mut out, &lang, &fence_body);
        st.code_blocks += 1;
    }
    flush_para(&mut out, &mut para, &mut st);
    close_list(&mut out, &mut list);

    (out, st)
}

fn flush_para(out: &mut String, para: &mut Vec<&str>, st: &mut StorageStats) {
    if para.is_empty() {
        return;
    }
    let joined = para.join(" ");
    out.push_str("<p>");
    out.push_str(&inline(&joined, st));
    out.push_str("</p>\n");
    para.clear();
}

fn close_list(out: &mut String, list: &mut Option<ListKind>) {
    if let Some(kind) = list.take() {
        out.push_str(kind.close());
        out.push('\n');
    }
}

fn emit_code(out: &mut String, lang: &str, body: &[&str]) {
    out.push_str("<ac:structured-macro ac:name=\"code\">");
    if !lang.is_empty() {
        out.push_str("<ac:parameter ac:name=\"language\">");
        out.push_str(&xml_escape(lang));
        out.push_str("</ac:parameter>");
    }
    out.push_str("<ac:plain-text-body><![CDATA[");
    out.push_str(&cdata_guard(&body.join("\n")));
    out.push_str("]]></ac:plain-text-body></ac:structured-macro>\n");
}

/// Split every `]]>` in a CDATA body across two sections — otherwise a fenced
/// block containing `]]>` terminates the section early and the page is
/// unparseable XHTML.
fn cdata_guard(s: &str) -> String {
    s.replace("]]>", "]]]]><![CDATA[>")
}

/// `#{1,6} text` → `(level, text)`. A space after the hashes is required.
fn heading(t: &str) -> Option<(usize, &str)> {
    let hashes = t.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = t.get(hashes..)?.strip_prefix(' ')?;
    Some((hashes, rest.trim()))
}

/// `- ` / `* ` / `+ ` → unordered, `N. ` → ordered.
fn list_item(t: &str) -> Option<(ListKind, &str)> {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = t.strip_prefix(marker) {
            return Some((ListKind::Ul, rest.trim()));
        }
    }
    let digits = t.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 {
        if let Some(rest) = t.get(digits..).and_then(|r| r.strip_prefix(". ")) {
            return Some((ListKind::Ol, rest.trim()));
        }
    }
    None
}

/// Single left-to-right inline pass: code span, link, strong, em, else escape.
/// An unmatched marker stays literal text.
fn inline(s: &str, st: &mut StorageStats) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let b = s.as_bytes();
    let mut i = 0usize;

    while i < b.len() {
        match b[i] {
            // 1. `x` — contents are NOT recursed into.
            b'`' => {
                if let Some(end) = find_from(s, i + 1, "`") {
                    out.push_str("<code>");
                    out.push_str(&xml_escape(&s[i + 1..end]));
                    out.push_str("</code>");
                    st.inline_code += 1;
                    i = end + 1;
                    continue;
                }
            }
            // 2. [text](href)
            b'[' => {
                if let Some((text, href, next)) = link_at(s, i) {
                    out.push_str("<a href=\"");
                    out.push_str(&xml_escape(href));
                    out.push_str("\">");
                    out.push_str(&inline(text, st));
                    out.push_str("</a>");
                    st.links += 1;
                    i = next;
                    continue;
                }
            }
            // 3a. **x**  (checked before single-`*` emphasis)
            b'*' if b.get(i + 1) == Some(&b'*') => {
                if let Some(end) = find_from(s, i + 2, "**") {
                    if end > i + 2 {
                        out.push_str("<strong>");
                        out.push_str(&inline(&s[i + 2..end], st));
                        out.push_str("</strong>");
                        i = end + 2;
                        continue;
                    }
                }
            }
            // 3b. *x* / _x_
            b'*' | b'_' => {
                let marker = if b[i] == b'*' { "*" } else { "_" };
                if let Some(end) = find_from(s, i + 1, marker) {
                    if end > i + 1 {
                        out.push_str("<em>");
                        out.push_str(&inline(&s[i + 1..end], st));
                        out.push_str("</em>");
                        i = end + 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        // 4. literal, escaped (UTF-8 safe: advance by a whole char).
        match s[i..].chars().next() {
            Some(ch) => {
                push_escaped(&mut out, ch);
                i += ch.len_utf8();
            }
            None => break,
        }
    }
    out
}

/// `s.find(pat)` restricted to `s[from..]`, returned as an absolute index.
fn find_from(s: &str, from: usize, pat: &str) -> Option<usize> {
    s.get(from..).and_then(|t| t.find(pat)).map(|p| p + from)
}

/// Parse `[text](href)` starting at `i` (which must be the `[`).
/// Returns the text, the href, and the index just past the closing `)`.
fn link_at(s: &str, i: usize) -> Option<(&str, &str, usize)> {
    let rest = s.get(i + 1..)?;
    let close = rest.find(']')?;
    let text = rest.get(..close)?;
    let after = rest.get(close + 1..)?.strip_prefix('(')?;
    let href_start = i + 1 + close + 2;
    let end = after.find(')')?;
    let href = after.get(..end)?;
    Some((text, href, href_start + end + 1))
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        push_escaped(&mut out, c);
    }
    out
}

/// `&` first — otherwise the ampersands of the other entities get re-escaped.
fn push_escaped(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        _ => out.push(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confluence::storage_to_markdown;

    // ---- (a) shape ----

    #[test]
    fn heading_becomes_hn() {
        let s = markdown_to_storage("# Overview\n### Deep");
        assert!(s.contains("<h1>Overview</h1>"), "got: {s:?}");
        assert!(s.contains("<h3>Deep</h3>"), "got: {s:?}");
    }

    #[test]
    fn fenced_block_becomes_code_macro_with_language() {
        let s = markdown_to_storage("```go\nfunc getUser() {}\n```");
        assert!(s.contains(r#"ac:name="code""#), "got: {s:?}");
        assert!(
            s.contains(r#"<ac:parameter ac:name="language">go</ac:parameter>"#),
            "got: {s:?}"
        );
        assert!(s.contains("<![CDATA[func getUser() {}]]>"), "got: {s:?}");
    }

    #[test]
    fn fenced_block_without_language_omits_parameter() {
        let s = markdown_to_storage("```\nplain\n```");
        assert!(s.contains(r#"ac:name="code""#), "got: {s:?}");
        assert!(!s.contains("ac:name=\"language\""), "got: {s:?}");
    }

    #[test]
    fn inline_code_becomes_code_element() {
        let (s, st) = markdown_to_storage_with_stats("The `OrderService` handles it.");
        assert!(s.contains("<code>OrderService</code>"), "got: {s:?}");
        assert_eq!(st.inline_code, 1);
    }

    #[test]
    fn link_becomes_anchor() {
        let (s, st) = markdown_to_storage_with_stats("See [api.go](https://x/api.go).");
        assert!(
            s.contains(r#"<a href="https://x/api.go">api.go</a>"#),
            "got: {s:?}"
        );
        assert_eq!(st.links, 1);
    }

    #[test]
    fn list_items_group_into_one_ul() {
        let s = markdown_to_storage("- one\n- two\n- three");
        assert_eq!(s.matches("<ul>").count(), 1, "got: {s:?}");
        assert_eq!(s.matches("</ul>").count(), 1, "got: {s:?}");
        assert_eq!(s.matches("<li>").count(), 3, "got: {s:?}");
        assert!(s.contains("<li>one</li><li>two</li><li>three</li>"), "got: {s:?}");
    }

    #[test]
    fn ordered_list_becomes_ol_and_a_blank_line_closes_it() {
        let s = markdown_to_storage("1. one\n2. two\n\ntail");
        assert!(s.contains("<ol><li>one</li><li>two</li></ol>"), "got: {s:?}");
        assert!(s.contains("<p>tail</p>"), "got: {s:?}");
    }

    #[test]
    fn xml_special_chars_escaped() {
        let s = markdown_to_storage("a < b & c");
        assert!(s.contains("a &lt; b &amp; c"), "got: {s:?}");
        assert!(!s.contains("a < b & c"), "got: {s:?}");
    }

    #[test]
    fn cdata_terminator_is_split() {
        let s = markdown_to_storage("```\na ]]> b\n```");
        assert!(s.contains("]]]]><![CDATA[>"), "got: {s:?}");
        // No bare terminator inside the body — that would end the section early.
        assert!(!s.contains("a ]]> b"), "got: {s:?}");
        // …and the two sections still read back as the original text.
        let md = storage_to_markdown(&s);
        assert!(md.contains("a ]]> b"), "got: {md:?}");
    }

    #[test]
    fn strong_and_em_and_unmatched_markers_stay_literal() {
        let s = markdown_to_storage("**bold** and _soft_ and a lone * star");
        assert!(s.contains("<strong>bold</strong>"), "got: {s:?}");
        assert!(s.contains("<em>soft</em>"), "got: {s:?}");
        assert!(s.contains("lone * star"), "got: {s:?}");
    }

    #[test]
    fn unterminated_fence_still_emits_its_text() {
        let s = markdown_to_storage("```go\nnever closed");
        assert!(s.contains("never closed"), "got: {s:?}");
        assert!(s.contains(r#"ac:name="code""#), "got: {s:?}");
    }

    // ---- (b) the real oracle: round-trip through the existing reader.
    //      These four mirror confluence.rs's tests one-for-one. ----

    #[test]
    fn roundtrip_preserves_code_spans() {
        let md = storage_to_markdown(&markdown_to_storage("The `OrderService` handles it."));
        assert!(md.contains("`OrderService`"), "got: {md:?}");
    }

    #[test]
    fn roundtrip_preserves_headings() {
        let md = storage_to_markdown(&markdown_to_storage("# Overview\n\n## Details"));
        assert!(md.contains("# Overview"), "got: {md:?}");
        assert!(md.contains("## Details"), "got: {md:?}");
    }

    #[test]
    fn roundtrip_preserves_fenced_language() {
        let md = storage_to_markdown(&markdown_to_storage("```go\nfunc getUser() {}\n```"));
        assert!(md.contains("```go"), "got: {md:?}");
        assert!(md.contains("func getUser() {}"), "got: {md:?}");
    }

    #[test]
    fn roundtrip_preserves_links() {
        let src = "See [api.go](https://github.com/acme/repo/blob/main/api.go).";
        let md = storage_to_markdown(&markdown_to_storage(src));
        assert!(
            md.contains("[api.go](https://github.com/acme/repo/blob/main/api.go)"),
            "got: {md:?}"
        );
    }

    #[test]
    fn roundtrip_realistic_page_and_stats() {
        let src = "# Ordering\n\nThe `OrderService` validates orders. See [api.go](https://x/api.go).\n\n```java\norderRepository.save(order);\n```\n";
        let (storage, st) = markdown_to_storage_with_stats(src);
        assert_eq!(
            st,
            StorageStats { headings: 1, code_blocks: 1, links: 1, inline_code: 1 }
        );
        let md = storage_to_markdown(&storage);
        assert!(md.contains("# Ordering"), "got: {md:?}");
        assert!(md.contains("`OrderService`"), "got: {md:?}");
        assert!(md.contains("[api.go](https://x/api.go)"), "got: {md:?}");
        assert!(md.contains("```java"), "got: {md:?}");
        assert!(md.contains("orderRepository.save(order);"), "got: {md:?}");
    }

    #[test]
    fn empty_input_is_empty_output() {
        let (s, st) = markdown_to_storage_with_stats("");
        assert_eq!(s, "");
        assert_eq!(st, StorageStats::default());
    }
}
