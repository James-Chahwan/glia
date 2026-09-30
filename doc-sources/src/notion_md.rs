//! Notion blocks -> Markdown (CE.4e): the pure half of the Notion adapter.
//!
//! [`blocks_to_markdown`] renders a page's top-level blocks, as the Notion API
//! serves them (`GET /v1/blocks/{id}/children`, API version 2025-09-03), into
//! the Markdown the doc linker reads. Code spans survive as `` `x` `` and code
//! blocks as fenced blocks, so the backtick-identifier linker fires exactly as
//! it does for Confluence and wiki pages.
//!
//! Rendered here: paragraph, heading_1 / heading_2 / heading_3, bulleted and
//! numbered list items, quote, code and divider. Any other block type is
//! skipped and counted in [`NotionStats::unsupported`]; nested children are
//! not read (CE.4f renders nesting and the remaining types).
//!
//! Rich text keeps its annotations: code -> `` `x` ``, bold -> `**x**`,
//! italic -> `*x*`, strikethrough -> `~~x~~`, a link -> `[x](href)`, nested in
//! that order (`**`code`**`); mention and equation spans render their plain
//! text.

use serde_json::Value;

use crate::notion::NotionStats;

/// Render `blocks` (top-level, document order) as Markdown, counting every
/// block in `stats.blocks` and every unrendered type in `stats.unsupported`.
/// Blocks are separated by a blank line, except consecutive list items, which
/// stay one list. A block with no text (an empty paragraph) renders nothing.
pub fn blocks_to_markdown(blocks: &[Value], stats: &mut NotionStats) -> String {
    let mut out = String::new();
    let mut prev_item = false;
    for block in blocks {
        stats.blocks += 1;
        let kind = block["type"].as_str().unwrap_or("");
        let data = &block[kind];
        let (text, item) = match kind {
            "paragraph" => (rich_text(&data["rich_text"]), false),
            "heading_1" => (heading("#", data), false),
            "heading_2" => (heading("##", data), false),
            "heading_3" => (heading("###", data), false),
            "bulleted_list_item" => (list_item("- ", data), true),
            "numbered_list_item" => (list_item("1. ", data), true),
            "quote" => (quote(data), false),
            "code" => (code_block(data), false),
            "divider" => ("---".to_string(), false),
            other => {
                let key = if other.is_empty() { "(untyped)" } else { other };
                *stats.unsupported.entry(key.to_string()).or_default() += 1;
                continue;
            }
        };
        if text.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str(if prev_item && item { "\n" } else { "\n\n" });
        }
        out.push_str(&text);
        prev_item = item;
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// A rich text array as Markdown, each span with its annotations and link.
pub fn rich_text(spans: &Value) -> String {
    spans
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(span)
        .collect()
}

/// A rich text array's plain text, joined verbatim (no annotations).
fn plain_text(spans: &Value) -> String {
    spans
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|s| {
            s["plain_text"]
                .as_str()
                .or_else(|| s["text"]["content"].as_str())
                .unwrap_or("")
        })
        .collect()
}

fn span(s: &Value) -> String {
    let text = s["plain_text"]
        .as_str()
        .or_else(|| s["text"]["content"].as_str())
        .unwrap_or("");
    if matches!(s["type"].as_str(), Some("mention" | "equation")) {
        return text.to_string();
    }
    // Emphasis markers must hug the text: `**x** ` renders, `**x **` does not.
    let core = text.trim();
    if core.is_empty() {
        return text.to_string();
    }
    let start = text.len() - text.trim_start().len();
    let (lead, trail) = (&text[..start], &text[start + core.len()..]);
    let on = |name: &str| s["annotations"][name].as_bool() == Some(true);
    let mut md = if on("code") {
        code_span(core)
    } else {
        core.to_string()
    };
    if on("strikethrough") {
        md = format!("~~{md}~~");
    }
    if on("italic") {
        md = format!("*{md}*");
    }
    if on("bold") {
        md = format!("**{md}**");
    }
    let href = s["href"]
        .as_str()
        .or_else(|| s["text"]["link"]["url"].as_str())
        .filter(|h| !h.is_empty());
    if let Some(href) = href {
        md = if href.contains([' ', '(', ')']) {
            format!("[{md}](<{href}>)")
        } else {
            format!("[{md}]({href})")
        };
    }
    format!("{lead}{md}{trail}")
}

/// `x` in a backtick fence one longer than its longest backtick run, padded
/// with a space when it starts or ends with a backtick.
fn code_span(text: &str) -> String {
    let fence = "`".repeat(longest_run(text, '`') + 1);
    if text.starts_with('`') || text.ends_with('`') {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}

fn longest_run(text: &str, c: char) -> usize {
    let (mut best, mut run) = (0, 0);
    for ch in text.chars() {
        run = if ch == c { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

/// `# text` at the given level, on one line.
fn heading(marks: &str, data: &Value) -> String {
    let text = rich_text(&data["rich_text"]);
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        String::new()
    } else {
        format!("{marks} {one_line}")
    }
}

/// `- text` / `1. text`, continuation lines indented under the marker.
fn list_item(marker: &str, data: &Value) -> String {
    let text = rich_text(&data["rich_text"]);
    if text.trim().is_empty() {
        return String::new();
    }
    let indent = " ".repeat(marker.len());
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 {
            out.push_str(marker);
        } else {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(&indent);
            }
        }
        out.push_str(line);
    }
    out
}

/// `> text`, every line quoted.
fn quote(data: &Value) -> String {
    let text = rich_text(&data["rich_text"]);
    if text.trim().is_empty() {
        return String::new();
    }
    text.lines()
        .map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A fenced block: the language as the info string (`plain text` -> `text`,
/// spaces as `-`) and the rich text joined verbatim, fenced one backtick
/// longer than any run inside it.
fn code_block(data: &Value) -> String {
    let body = plain_text(&data["rich_text"]);
    let language = match data["language"].as_str().map(str::trim) {
        Some("plain text") | None => "text".to_string(),
        Some(l) => l.split_whitespace().collect::<Vec<_>>().join("-"),
    };
    let fence = "`".repeat(longest_run(&body, '`').max(2) + 1);
    let newline = if body.ends_with('\n') || body.is_empty() { "" } else { "\n" };
    format!("{fence}{language}\n{body}{newline}{fence}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(content: &str, annotations: Value, href: Option<&str>) -> Value {
        let mut ann = json!({
            "bold": false, "italic": false, "strikethrough": false,
            "underline": false, "code": false, "color": "default",
        });
        if let (Some(a), Some(extra)) = (ann.as_object_mut(), annotations.as_object()) {
            for (k, v) in extra {
                a.insert(k.clone(), v.clone());
            }
        }
        json!({
            "type": "text",
            "text": { "content": content, "link": href.map(|h| json!({ "url": h })) },
            "annotations": ann,
            "plain_text": content,
            "href": href,
        })
    }

    fn block(kind: &str, spans: Vec<Value>) -> Value {
        json!({ "object": "block", "type": kind, kind: { "rich_text": spans, "color": "default" } })
    }

    #[test]
    fn rich_text_annotations() {
        let spans = json!([
            text("code", json!({ "bold": true, "code": true }), None),
            text(" and ", json!({}), None),
            text("x", json!({}), Some("https://y")),
        ]);
        assert_eq!(rich_text(&spans), "**`code`** and [x](https://y)");
        let spans = json!([
            text("gone ", json!({ "strikethrough": true, "italic": true }), None),
            text("a`b", json!({ "code": true }), None),
        ]);
        assert_eq!(rich_text(&spans), "*~~gone~~* ``a`b``");
        let mention = json!([{ "type": "mention", "plain_text": "@Ada",
            "annotations": { "bold": true }, "href": "https://www.notion.so/u" }]);
        assert_eq!(rich_text(&mention), "@Ada");
        let equation = json!([{ "type": "equation", "plain_text": "E = mc^2",
            "equation": { "expression": "E = mc^2" } }]);
        assert_eq!(rich_text(&equation), "E = mc^2");
    }

    #[test]
    fn blocks_render_and_lists_stay_together() {
        let mut stats = NotionStats::default();
        let blocks = vec![
            block("heading_1", vec![text("Orders", json!({}), None)]),
            block("paragraph", vec![text("Calls ", json!({}), None),
                text("OrderService.place", json!({ "code": true }), None)]),
            block("paragraph", vec![]),
            block("bulleted_list_item", vec![text("one", json!({}), None)]),
            block("bulleted_list_item", vec![text("two\nlines", json!({}), None)]),
            block("numbered_list_item", vec![text("first", json!({}), None)]),
            block("quote", vec![text("said\nthis", json!({}), None)]),
            json!({ "type": "divider", "divider": {} }),
            json!({ "type": "code", "code": { "language": "plain text",
                "rich_text": [text("a ```", json!({ "bold": true }), None)] } }),
            block("heading_3", vec![text("Deep", json!({}), None)]),
        ];
        let md = blocks_to_markdown(&blocks, &mut stats);
        assert_eq!(
            md,
            "# Orders\n\nCalls `OrderService.place`\n\n- one\n- two\n  lines\n1. first\n\n\
             > said\n> this\n\n---\n\n````text\na ```\n````\n\n### Deep\n"
        );
        assert_eq!(stats.blocks, 10);
        assert!(stats.unsupported.is_empty());
    }

    #[test]
    fn unsupported_types_are_counted_not_rendered() {
        let mut stats = NotionStats::default();
        let blocks = vec![
            json!({ "type": "image", "image": { "type": "external", "external": { "url": "https://i" } } }),
            json!({ "type": "image", "image": {} }),
            json!({ "type": "toggle", "toggle": { "rich_text": [] }, "has_children": true }),
            json!({ "object": "block" }),
        ];
        assert_eq!(blocks_to_markdown(&blocks, &mut stats), "");
        assert_eq!(stats.blocks, 4);
        assert_eq!(
            stats.unsupported.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(),
            [("(untyped)", 1), ("image", 2), ("toggle", 1)]
        );
    }
}
