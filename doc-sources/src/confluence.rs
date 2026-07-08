//! Confluence **storage-format** (XHTML) → markdown.
//!
//! Storage format is XHTML with Confluence macros in the `ac:`/`ri:` namespaces.
//! We convert the linker-critical structure:
//!   - `<h1>`…`<h6>`                → `#`…`######` (drives DOC_SECTION chunking)
//!   - inline `<code>`             → `` `…` ``  (so the backtick linker fires)
//!   - `<ac:structured-macro ac:name="code">` + `<ac:plain-text-body>` (CDATA)
//!                                 → fenced ```lang block (language from the
//!                                   `language` `<ac:parameter>`)
//!   - `<a href>` / `<ac:link>`    → `[text](href)` (source-file links = a Strong
//!                                   doc→code signal)
//!   - `<p>` / `<li>` / `<strong>` / `<em>` / `<br>`
//!
//! The point is **fidelity of code spans** — a `` `OrderService` `` in a
//! Confluence page must survive as backticks so the linker treats it as a Strong
//! code-formatted mention (handoff Tier-4 linker policy).

use quick_xml::events::Event;
use quick_xml::reader::Reader;

/// Strip a namespace prefix (`ac:name` → `name`).
fn local(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// Fetch an attribute value (key may carry a namespace, e.g. `ac:name`).
fn attr(e: &quick_xml::events::BytesStart, key: &[u8]) -> Option<String> {
    e.attributes().flatten().find_map(|a| {
        if a.key.as_ref() == key {
            a.unescape_value().ok().map(|v| v.into_owned())
        } else {
            None
        }
    })
}

/// Where the current text run should go.
enum Sink {
    Out,
    Link,
    CodeParam,
    CodeBody,
}

/// Convert Confluence storage-format XHTML to markdown, preserving code spans.
pub fn storage_to_markdown(xhtml: &str) -> String {
    let mut reader = Reader::from_str(xhtml);
    let mut out = String::new();

    // link buffering
    let mut link_href: Option<String> = None;
    let mut link_buf = String::new();
    // code macro buffering
    let mut in_code_macro = false;
    let mut code_lang = String::new();
    let mut code_body = String::new();
    let mut in_plain_body = false;
    let mut param_is_language = false;
    let mut in_param = false;
    let mut param_buf = String::new();

    let sink = |in_param: bool,
                in_plain_body: bool,
                link_href: &Option<String>|
     -> Sink {
        if in_param {
            Sink::CodeParam
        } else if in_plain_body {
            Sink::CodeBody
        } else if link_href.is_some() {
            Sink::Link
        } else {
            Sink::Out
        }
    };

    loop {
        match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,

            Ok(Event::Start(e)) => match local(e.name().as_ref()) {
                b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" => {
                    let level = (e.name().as_ref()[local_off(e.name().as_ref()) + 1] - b'0') as usize;
                    out.push('\n');
                    for _ in 0..level {
                        out.push('#');
                    }
                    out.push(' ');
                }
                b"code" => out.push('`'),
                b"a" => {
                    link_href = attr(&e, b"href").or(Some(String::new()));
                    link_buf.clear();
                }
                b"link" => {
                    // <ac:link><ri:page ri:content-title="X"/> … — approximate as
                    // a bracketed reference; href unknown, keep the text.
                    link_href = Some(String::new());
                    link_buf.clear();
                }
                b"strong" | b"b" => out.push_str("**"),
                b"em" | b"i" => out.push('*'),
                b"li" => out.push_str("\n- "),
                b"structured-macro" => {
                    if attr(&e, b"ac:name").as_deref() == Some("code") {
                        in_code_macro = true;
                        code_lang.clear();
                        code_body.clear();
                    }
                }
                b"parameter" if in_code_macro => {
                    in_param = true;
                    param_is_language = attr(&e, b"ac:name").as_deref() == Some("language");
                    param_buf.clear();
                }
                b"plain-text-body" if in_code_macro => {
                    in_plain_body = true;
                    code_body.clear();
                }
                _ => {}
            },

            Ok(Event::End(e)) => match local(e.name().as_ref()) {
                b"h1" | b"h2" | b"h3" | b"h4" | b"h5" | b"h6" | b"p" => out.push_str("\n\n"),
                b"code" => out.push('`'),
                b"a" | b"link" => {
                    let text = link_buf.trim().to_string();
                    match link_href.take() {
                        Some(href) if !href.is_empty() => {
                            out.push('[');
                            out.push_str(&text);
                            out.push_str("](");
                            out.push_str(&href);
                            out.push(')');
                        }
                        _ => out.push_str(&text),
                    }
                    link_buf.clear();
                }
                b"strong" | b"b" => out.push_str("**"),
                b"em" | b"i" => out.push('*'),
                b"structured-macro" if in_code_macro => {
                    out.push_str("\n```");
                    out.push_str(code_lang.trim());
                    out.push('\n');
                    out.push_str(code_body.trim_matches('\n'));
                    out.push_str("\n```\n\n");
                    in_code_macro = false;
                }
                b"parameter" if in_param => {
                    if param_is_language {
                        code_lang = param_buf.trim().to_string();
                    }
                    in_param = false;
                }
                b"plain-text-body" => in_plain_body = false,
                _ => {}
            },

            Ok(Event::Text(t)) => {
                let s = t.unescape().unwrap_or_default();
                match sink(in_param, in_plain_body, &link_href) {
                    Sink::Out => out.push_str(&s),
                    Sink::Link => link_buf.push_str(&s),
                    Sink::CodeParam => param_buf.push_str(&s),
                    Sink::CodeBody => code_body.push_str(&s),
                }
            }
            Ok(Event::CData(t)) => {
                let bytes = t.into_inner();
                let s = String::from_utf8_lossy(&bytes);
                match sink(in_param, in_plain_body, &link_href) {
                    Sink::CodeBody => code_body.push_str(&s),
                    Sink::CodeParam => param_buf.push_str(&s),
                    Sink::Link => link_buf.push_str(&s),
                    Sink::Out => out.push_str(&s),
                }
            }
            Ok(Event::Empty(e)) => {
                if local(e.name().as_ref()) == b"br" {
                    out.push('\n');
                }
            }
            Ok(_) => {}
        }
    }

    normalize_blank_lines(&out)
}

/// Offset of the local-name start (after any `ns:` prefix) within a tag name.
fn local_off(name: &[u8]) -> usize {
    name.iter().rposition(|&b| b == b':').map_or(0, |i| i + 1)
    // NOTE: h1..h6 tags are never namespaced, so this returns 0 for them; the
    // level byte is then name[1]. Kept general for safety.
}

/// Collapse 3+ consecutive newlines to a blank-line separator; trim ends.
fn normalize_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut nls = 0usize;
    for c in s.chars() {
        if c == '\n' {
            nls += 1;
            if nls <= 2 {
                out.push('\n');
            }
        } else {
            nls = 0;
            out.push(c);
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_code_becomes_backticks() {
        // The linker-critical case: an inline <code> span must survive as
        // backticks so the doc→code linker treats it as a Strong mention.
        let md = storage_to_markdown("<p>The <code>OrderService</code> handles it.</p>");
        assert!(md.contains("`OrderService`"), "got: {md:?}");
    }

    #[test]
    fn headings_drive_chunking() {
        let md = storage_to_markdown("<h1>Overview</h1><h2>Details</h2>");
        assert!(md.contains("# Overview"), "got: {md:?}");
        assert!(md.contains("## Details"), "got: {md:?}");
    }

    #[test]
    fn code_macro_becomes_fenced_block_with_language() {
        let x = r#"<ac:structured-macro ac:name="code">
            <ac:parameter ac:name="language">go</ac:parameter>
            <ac:plain-text-body><![CDATA[func getUser() {}]]></ac:plain-text-body>
          </ac:structured-macro>"#;
        let md = storage_to_markdown(x);
        assert!(md.contains("```go"), "got: {md:?}");
        assert!(md.contains("func getUser() {}"), "got: {md:?}");
    }

    #[test]
    fn source_link_preserved() {
        let md = storage_to_markdown(
            r#"<p>See <a href="https://github.com/acme/repo/blob/main/api.go">api.go</a>.</p>"#,
        );
        assert!(
            md.contains("[api.go](https://github.com/acme/repo/blob/main/api.go)"),
            "got: {md:?}"
        );
    }

    #[test]
    fn realistic_page_preserves_all_code_spans() {
        let x = r#"<h1>Ordering</h1>
            <p>The <code>OrderService</code> validates and persists orders.
            It calls <code>PaymentGateway.charge</code>.</p>
            <ac:structured-macro ac:name="code"><ac:parameter ac:name="language">java</ac:parameter>
            <ac:plain-text-body><![CDATA[orderRepository.save(order);]]></ac:plain-text-body></ac:structured-macro>"#;
        let md = storage_to_markdown(x);
        assert!(md.contains("# Ordering"));
        assert!(md.contains("`OrderService`"));
        assert!(md.contains("`PaymentGateway.charge`"));
        assert!(md.contains("```java"));
        assert!(md.contains("orderRepository.save(order);"));
    }
}
