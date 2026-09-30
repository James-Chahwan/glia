//! Notion blocks -> Markdown (CE.4e, CE.4f): the pure half of the Notion
//! adapter.
//!
//! [`blocks_to_markdown`] renders a page's blocks, as the Notion API serves
//! them (`GET /v1/blocks/{id}/children`, API version 2025-09-03), into the
//! Markdown the doc linker reads. A block whose children the fetcher read
//! ([`crate::notion`]) carries them as a `children` array; they render nested
//! under it. Code spans survive as `` `x` `` and code blocks as fenced blocks
//! (indented under a list item or toggle), so the backtick-identifier linker
//! fires exactly as it does for Confluence and wiki pages.
//!
//! Rendered: paragraph; heading_1 / heading_2 / heading_3 (a toggleable
//! heading's children follow it as its section); bulleted / numbered list
//! items, `to_do` (`- [ ] ` / `- [x] `) and `toggle` (`- <summary>`), their
//! children indented under the marker (two spaces; three under `1. `, the
//! marker's width, so a numbered item's children stay inside it); quote and
//! callout (`> <emoji> <text>`), children quoted; code; divider; table (a
//! Markdown table of its `table_row`s, the first row the header when
//! `has_column_header`, else an empty header row); bookmark / embed /
//! link_preview (`[<caption or url>](url)`); image / file / pdf / video /
//! audio (`[<caption or file name>](url)`, never downloaded, and a
//! Notion-hosted file's signed query string dropped); equation
//! (`$<expression>$`); child_page (`- [<title>](https://www.notion.so/<id>)`);
//! child_database (its title); link_to_page (the linked id as a link).
//! `synced_block`, `column_list` and `column` are transparent: their children
//! render in their place. `breadcrumb` and `table_of_contents` carry no
//! content and are dropped ([`NotionStats::dropped`]); any other type is
//! skipped and counted in [`NotionStats::unsupported`], its children (when
//! read) rendered in its place.
//!
//! Rich text keeps its annotations: code -> `` `x` ``, bold -> `**x**`,
//! italic -> `*x*`, strikethrough -> `~~x~~`, a link -> `[x](href)`, nested in
//! that order (`**`code`**`); mention and equation spans render their plain
//! text.

use serde_json::Value;

use crate::notion::NotionStats;

/// Where a Notion page id links to in rendered Markdown: its public
/// `notion.so` URL (never fetched).
const NOTION_PAGE_BASE: &str = "https://www.notion.so/";

/// Render `blocks` (document order, each with its fetched `children`) as
/// Markdown, counting every block, nested ones included, in `stats.blocks`
/// and every unrendered type in `stats.unsupported`. Blocks are separated by a
/// blank line, except consecutive list items, which stay one list. A block
/// with no text and no children (an empty paragraph) renders nothing.
pub fn blocks_to_markdown(blocks: &[Value], stats: &mut NotionStats) -> String {
    let mut out = render(blocks, stats);
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// One rendered block: its Markdown (no trailing newline) and whether it is a
/// list line, which joins the list line before it with a single newline.
struct Part {
    text: String,
    item: bool,
}

/// `blocks` as Markdown with no trailing newline.
fn render(blocks: &[Value], stats: &mut NotionStats) -> String {
    let mut parts = Vec::new();
    emit(blocks, stats, &mut parts);
    let mut out = String::new();
    let mut prev_item = false;
    for part in parts {
        if part.text.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push_str(if prev_item && part.item { "\n" } else { "\n\n" });
        }
        out.push_str(&part.text);
        prev_item = part.item;
    }
    out
}

/// Push each block's [`Part`], a transparent container's children in its place.
fn emit(blocks: &[Value], stats: &mut NotionStats, parts: &mut Vec<Part>) {
    for block in blocks {
        stats.blocks += 1;
        let kind = block["type"].as_str().unwrap_or("");
        let data = &block[kind];
        let kids = block["children"].as_array().map(Vec::as_slice).unwrap_or_default();
        let (text, item) = match kind {
            "paragraph" => (nest(&rich_text(&data["rich_text"]), kids, stats), false),
            "heading_1" | "heading_2" | "heading_3" => {
                let marks = match kind {
                    "heading_1" => "#",
                    "heading_2" => "##",
                    _ => "###",
                };
                parts.push(Part { text: heading(marks, data), item: false });
                emit(kids, stats, parts);
                continue;
            }
            "bulleted_list_item" | "toggle" => list_block("- ", 2, data, kids, stats),
            "numbered_list_item" => list_block("1. ", 3, data, kids, stats),
            "to_do" => {
                let checked = data["checked"].as_bool() == Some(true);
                list_block(if checked { "- [x] " } else { "- [ ] " }, 2, data, kids, stats)
            }
            "quote" => (quoted(&quote(&rich_text(&data["rich_text"])), kids, stats), false),
            "callout" => (quoted(&callout(data), kids, stats), false),
            "code" => (nest(&code_block(data), kids, stats), false),
            "divider" => (nest("---", kids, stats), false),
            "table" => (table(data, kids, stats), false),
            "bookmark" | "embed" | "link_preview" => (nest(&bookmark(data), kids, stats), false),
            "image" | "file" | "pdf" | "video" | "audio" => {
                (nest(&file_link(data), kids, stats), false)
            }
            "equation" => {
                let expr = data["expression"].as_str().unwrap_or("").trim();
                let text = if expr.is_empty() { String::new() } else { format!("${expr}$") };
                (nest(&text, kids, stats), false)
            }
            "child_page" => {
                let title = one_line(data["title"].as_str().unwrap_or(""));
                let title = if title.is_empty() { "Untitled".to_string() } else { title };
                let id = block["id"].as_str().unwrap_or("");
                let text = match page_url(id) {
                    Some(url) => format!("- {}", link(&title, &url)),
                    None => format!("- {}", escape_label(&title)),
                };
                (text, true)
            }
            "child_database" => (one_line(data["title"].as_str().unwrap_or("")), false),
            "link_to_page" => {
                let target = data["type"].as_str().unwrap_or("");
                let id = data[target].as_str().unwrap_or("");
                let text = page_url(id).map(|url| link(id, &url)).unwrap_or_default();
                (text, false)
            }
            "synced_block" | "column_list" | "column" => {
                emit(kids, stats, parts);
                continue;
            }
            "breadcrumb" | "table_of_contents" => {
                stats.dropped += 1;
                continue;
            }
            other => {
                let key = if other.is_empty() { "(untyped)" } else { other };
                *stats.unsupported.entry(key.to_string()).or_default() += 1;
                emit(kids, stats, parts);
                continue;
            }
        };
        parts.push(Part { text, item });
    }
}

/// A block that is not a list line (a paragraph, a code block), then its
/// children after a blank line, indented two spaces. With no text of its own
/// the children stand in its place, unindented.
fn nest(head: &str, kids: &[Value], stats: &mut NotionStats) -> String {
    let body = render(kids, stats);
    if body.is_empty() {
        head.to_string()
    } else if head.trim().is_empty() {
        body
    } else {
        format!("{head}\n\n{}", indent_lines(&body, "  "))
    }
}

/// A list line (`- `, `1. `, `- [ ] `, a toggle's `- `), its continuation
/// lines and children indented `width` spaces (the column its text starts
/// at), children on the next line. It stays a list line only with text of its
/// own; without, its children stand in its place.
fn list_block(
    marker: &str,
    width: usize,
    data: &Value,
    kids: &[Value],
    stats: &mut NotionStats,
) -> (String, bool) {
    let indent = " ".repeat(width);
    let head = list_item(marker, &indent, data);
    let body = render(kids, stats);
    if head.is_empty() {
        (body, false)
    } else if body.is_empty() {
        (head, true)
    } else {
        (format!("{head}\n{}", indent_lines(&body, &indent)), true)
    }
}

/// `head` (already quoted) with `kids` quoted under it, a `>` line between.
fn quoted(head: &str, kids: &[Value], stats: &mut NotionStats) -> String {
    let body = render(kids, stats);
    if body.is_empty() {
        return head.to_string();
    }
    let body = quote_lines(&body);
    if head.trim().is_empty() {
        body
    } else {
        format!("{head}\n>\n{body}")
    }
}

fn indent_lines(text: &str, indent: &str) -> String {
    text.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("{indent}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

fn quote_lines(text: &str) -> String {
    text.lines()
        .map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") })
        .collect::<Vec<_>>()
        .join("\n")
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
        md = format!("[{md}]({})", link_target(href));
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
    let text = one_line(&rich_text(&data["rich_text"]));
    if text.is_empty() {
        String::new()
    } else {
        format!("{marks} {text}")
    }
}

/// `- text` / `1. text`, continuation lines indented by `indent`.
fn list_item(marker: &str, indent: &str, data: &Value) -> String {
    let text = rich_text(&data["rich_text"]);
    if text.trim().is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        if i == 0 {
            out.push_str(marker);
        } else {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(indent);
            }
        }
        out.push_str(line);
    }
    out
}

/// `> text`, every line quoted.
fn quote(text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    quote_lines(text)
}

/// `> <emoji> text`, every line quoted; the icon only when it is an emoji (a
/// custom emoji or an icon image has no text form).
fn callout(data: &Value) -> String {
    let text = rich_text(&data["rich_text"]);
    let emoji = (data["icon"]["type"] == "emoji")
        .then(|| data["icon"]["emoji"].as_str())
        .flatten()
        .map(str::trim)
        .filter(|e| !e.is_empty());
    match emoji {
        Some(e) if text.trim().is_empty() => format!("> {e}"),
        Some(e) => quote(&format!("{e} {text}")),
        None => quote(&text),
    }
}

/// A table's `table_row` children as a Markdown table: the first row is the
/// header when `has_column_header`, else the header row is empty. Rows are
/// padded to the widest (or `table_width`); a `|` in a cell is escaped and a
/// line break becomes a space.
fn table(data: &Value, rows: &[Value], stats: &mut NotionStats) -> String {
    let mut cells: Vec<Vec<String>> = Vec::new();
    for row in rows {
        stats.blocks += 1;
        if row["type"] != "table_row" {
            let key = row["type"].as_str().filter(|k| !k.is_empty()).unwrap_or("(untyped)");
            *stats.unsupported.entry(key.to_string()).or_default() += 1;
            continue;
        }
        let row_cells = row["table_row"]["cells"].as_array().map(Vec::as_slice).unwrap_or_default();
        cells.push(row_cells.iter().map(table_cell).collect());
    }
    let declared = data["table_width"].as_u64().and_then(|w| usize::try_from(w).ok()).unwrap_or(0);
    let width = cells.iter().map(Vec::len).max().unwrap_or(0).max(declared);
    if width == 0 || cells.is_empty() {
        return String::new();
    }
    let header = if data["has_column_header"].as_bool() == Some(true) {
        cells.remove(0)
    } else {
        Vec::new()
    };
    let line = |row: &[String]| {
        let mut out = String::from("|");
        for i in 0..width {
            match row.get(i).map(String::as_str).filter(|c| !c.is_empty()) {
                Some(c) => {
                    out.push(' ');
                    out.push_str(c);
                    out.push_str(" |");
                }
                None => out.push_str(" |"),
            }
        }
        out
    };
    let mut lines = vec![line(&header), line(&vec!["---".to_string(); width])];
    lines.extend(cells.iter().map(|r| line(r)));
    lines.join("\n")
}

fn table_cell(spans: &Value) -> String {
    one_line(&rich_text(spans)).replace('|', "\\|")
}

/// `[<caption or url>](url)` for a bookmark, embed or link preview.
fn bookmark(data: &Value) -> String {
    let url = data["url"].as_str().map(str::trim).unwrap_or("");
    if url.is_empty() {
        return String::new();
    }
    let caption = one_line(&plain_text(&data["caption"]));
    link(if caption.is_empty() { url } else { &caption }, url)
}

/// `[<caption or file name>](url)` for an image, file, pdf, video or audio
/// block. The file is never downloaded. A Notion-hosted file's url is a
/// short-lived signed link: its query string (the signature) is dropped. A
/// file with no url (an API upload) renders its caption or name alone.
fn file_link(data: &Value) -> String {
    let url = match data["type"].as_str() {
        Some("external") => data["external"]["url"].as_str().map(str::to_string),
        Some("file") => data["file"]["url"]
            .as_str()
            .map(|u| u.split(['?', '#']).next().unwrap_or(u).to_string()),
        _ => None,
    }
    .map(|u| u.trim().to_string())
    .filter(|u| !u.is_empty());
    let caption = one_line(&plain_text(&data["caption"]));
    let name = data["name"]
        .as_str()
        .map(one_line)
        .filter(|n| !n.is_empty())
        .or_else(|| {
            url.as_deref()
                .map(|u| u.split(['?', '#']).next().unwrap_or(u))
                .and_then(|u| u.rsplit('/').next())
                .map(str::to_string)
                .filter(|n| !n.is_empty())
        });
    let label = if caption.is_empty() { name.unwrap_or_default() } else { caption };
    match url {
        Some(url) => link(if label.is_empty() { &url } else { &label }, &url),
        None => escape_label(&label),
    }
}

/// The `notion.so` URL of a page id (hyphens dropped), or `None` when the id
/// is not ASCII letters, digits and `-`.
fn page_url(id: &str) -> Option<String> {
    let id = id.trim();
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .then(|| format!("{NOTION_PAGE_BASE}{}", id.replace('-', "")))
}

/// `[label](href)`, the label's brackets escaped and the href in `<>` when it
/// holds a space or a parenthesis.
fn link(label: &str, href: &str) -> String {
    format!("[{}]({})", escape_label(label), link_target(href))
}

fn link_target(href: &str) -> String {
    if href.contains([' ', '(', ')']) {
        format!("<{href}>")
    } else {
        href.to_string()
    }
}

fn escape_label(label: &str) -> String {
    label.replace('[', "\\[").replace(']', "\\]")
}

/// `text` on one line, runs of whitespace as one space.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

    fn with_children(mut b: Value, kids: Vec<Value>) -> Value {
        b["has_children"] = json!(true);
        b["children"] = Value::Array(kids);
        b
    }

    fn plain(content: &str) -> Vec<Value> {
        vec![text(content, json!({}), None)]
    }

    #[test]
    fn unsupported_types_are_counted_not_rendered() {
        let mut stats = NotionStats::default();
        let blocks = vec![
            json!({ "type": "template", "template": { "rich_text": [] } }),
            json!({ "type": "template", "template": {} }),
            json!({ "type": "breadcrumb", "breadcrumb": {} }),
            json!({ "type": "table_of_contents", "table_of_contents": { "color": "default" } }),
            json!({ "object": "block" }),
        ];
        assert_eq!(blocks_to_markdown(&blocks, &mut stats), "");
        assert_eq!(stats.blocks, 5);
        assert_eq!(stats.dropped, 2, "breadcrumb and table_of_contents carry no content");
        assert_eq!(
            stats.unsupported.iter().map(|(k, v)| (k.as_str(), *v)).collect::<Vec<_>>(),
            [("(untyped)", 1), ("template", 2)]
        );

        // An unsupported block's children still render, in its place.
        let mut stats = NotionStats::default();
        let tab = with_children(
            json!({ "type": "tab", "tab": {} }),
            vec![block("paragraph", vec![text("Run", json!({}), None), text("make", json!({ "code": true }), None)])],
        );
        assert_eq!(blocks_to_markdown(&[tab], &mut stats), "Run`make`\n");
        assert_eq!((stats.blocks, stats.unsupported_total()), (2, 1));
    }

    #[test]
    fn nested_children_indent_under_their_list_line() {
        let mut stats = NotionStats::default();
        let code = json!({ "type": "code", "code": { "language": "rust",
            "rich_text": [text("fn place() {}", json!({}), None)] } });
        let blocks = vec![
            with_children(block("toggle", plain("Details")), vec![code]),
            with_children(
                block("bulleted_list_item", plain("a")),
                vec![with_children(
                    block("numbered_list_item", plain("b")),
                    vec![block("bulleted_list_item", plain("c"))],
                )],
            ),
            json!({ "type": "to_do", "to_do": { "rich_text": plain("ship"), "checked": true } }),
            json!({ "type": "to_do", "to_do": { "rich_text": plain("test"), "checked": false } }),
            with_children(block("paragraph", plain("Intro")), vec![block("paragraph", plain("indented"))]),
            with_children(block("toggle", vec![]), vec![block("paragraph", plain("no summary"))]),
        ];
        assert_eq!(
            blocks_to_markdown(&blocks, &mut stats),
            "- Details\n  ```rust\n  fn place() {}\n  ```\n- a\n  1. b\n     - c\n- [x] ship\n- [ ] test\n\n\
             Intro\n\n  indented\n\nno summary\n"
        );
        assert_eq!(stats.blocks, 11);
        assert!(stats.unsupported.is_empty());
    }

    #[test]
    fn quotes_callouts_and_containers() {
        let mut stats = NotionStats::default();
        let callout = json!({ "type": "callout", "callout": {
            "rich_text": plain("Heads up"), "icon": { "type": "emoji", "emoji": "💡" }, "color": "default" } });
        let blocks = vec![
            with_children(callout, vec![block("paragraph", vec![text("Use ", json!({}), None),
                text("OrderService", json!({ "code": true }), None)])]),
            with_children(block("quote", plain("said")), vec![block("bulleted_list_item", plain("x"))]),
            json!({ "type": "callout", "callout": { "rich_text": plain("No icon"),
                "icon": { "type": "external", "external": { "url": "https://i/x.png" } } } }),
            // An original synced block and a column layout: children in place.
            with_children(
                json!({ "type": "synced_block", "synced_block": { "synced_from": null } }),
                vec![block("paragraph", plain("shared"))],
            ),
            with_children(
                json!({ "type": "column_list", "column_list": {} }),
                vec![
                    with_children(json!({ "type": "column", "column": {} }), vec![block("paragraph", plain("left"))]),
                    with_children(json!({ "type": "column", "column": {} }), vec![block("paragraph", plain("right"))]),
                ],
            ),
            with_children(
                json!({ "type": "heading_2", "heading_2": { "rich_text": plain("Toggled"), "is_toggleable": true } }),
                vec![block("paragraph", plain("section body"))],
            ),
        ];
        assert_eq!(
            blocks_to_markdown(&blocks, &mut stats),
            "> 💡 Heads up\n>\n> Use `OrderService`\n\n> said\n>\n> - x\n\n> No icon\n\n\
             shared\n\nleft\n\nright\n\n## Toggled\n\nsection body\n"
        );
        assert_eq!(stats.blocks, 14);
        assert!(stats.unsupported.is_empty());
    }

    #[test]
    fn tables_render_with_or_without_a_header() {
        let row = |a: &str, b: &str| {
            json!({ "type": "table_row", "table_row": { "cells": [plain(a), plain(b)] } })
        };
        let mut stats = NotionStats::default();
        let with_header = with_children(
            json!({ "type": "table", "table": { "table_width": 2, "has_column_header": true } }),
            vec![row("h1", "h2"), row("a", "b|c")],
        );
        let without = with_children(
            json!({ "type": "table", "table": { "table_width": 3, "has_column_header": false } }),
            vec![row("x", "")],
        );
        assert_eq!(
            blocks_to_markdown(&[with_header, without], &mut stats),
            "| h1 | h2 |\n| --- | --- |\n| a | b\\|c |\n\n| | | |\n| --- | --- | --- |\n| x | | |\n"
        );
        assert_eq!(stats.blocks, 5);
    }

    #[test]
    fn links_media_and_pages() {
        let mut stats = NotionStats::default();
        let page_id = "1a2b3c4d-0000-4000-8000-00000000000a";
        let blocks = vec![
            json!({ "type": "bookmark", "bookmark": { "url": "https://ex.com/a", "caption": [] } }),
            json!({ "type": "embed", "embed": { "url": "https://ex.com/e", "caption": plain("The [spec]") } }),
            json!({ "type": "link_preview", "link_preview": { "url": "https://github.com/o/r/pull/1" } }),
            json!({ "type": "image", "image": { "type": "external", "caption": [],
                "external": { "url": "https://img.example/a b.png" } } }),
            json!({ "type": "file", "file": { "type": "file", "caption": [], "name": "design.pdf",
                "file": { "url": "https://s3.us-west-2.amazonaws.com/secure/f/design.pdf?X-Amz-Signature=abc",
                          "expiry_time": "2026-10-01T00:00:00.000Z" } } }),
            json!({ "type": "video", "video": { "type": "file_upload", "caption": plain("Demo"),
                "file_upload": { "id": "u1" } } }),
            json!({ "type": "equation", "equation": { "expression": "e^{i\\pi} + 1 = 0" } }),
            json!({ "type": "child_page", "id": page_id, "child_page": { "title": "Runbook" } }),
            json!({ "type": "child_database", "id": "d1", "child_database": { "title": "Incidents" } }),
            json!({ "type": "link_to_page", "link_to_page": { "type": "page_id", "page_id": page_id } }),
        ];
        assert_eq!(
            blocks_to_markdown(&blocks, &mut stats),
            "[https://ex.com/a](https://ex.com/a)\n\n[The \\[spec\\]](https://ex.com/e)\n\n\
             [https://github.com/o/r/pull/1](https://github.com/o/r/pull/1)\n\n\
             [a b.png](<https://img.example/a b.png>)\n\n\
             [design.pdf](https://s3.us-west-2.amazonaws.com/secure/f/design.pdf)\n\nDemo\n\n\
             $e^{i\\pi} + 1 = 0$\n\n\
             - [Runbook](https://www.notion.so/1a2b3c4d00004000800000000000000a)\n\nIncidents\n\n\
             [1a2b3c4d-0000-4000-8000-00000000000a](https://www.notion.so/1a2b3c4d00004000800000000000000a)\n"
        );
        assert_eq!(stats.blocks, 10);
        assert!(stats.unsupported.is_empty());
    }
}
