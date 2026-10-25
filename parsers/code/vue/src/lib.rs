//! Single-file components (FH.6): Vue `.vue`, Svelte `.svelte` and Astro
//! `.astro` files are markup wrapping script blocks. [`script_view`] keeps
//! every script block of the file (and Astro's leading `---` frontmatter) in
//! place and blanks everything else, byte for byte, so the TypeScript parser
//! reads one program whose byte offsets and line numbers are the file's own:
//! a POSITION cell names the file's real line, and the cross-cutting
//! extractors (which read the whole file) agree with the parse on every line.
//!
//! Template expressions (`{fn()}`, `@click=`, `on:click=`) are markup and are
//! not parsed here: a call only a template makes stays unseen.

pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use glia_core::RepoId;

/// The single-file component formats this crate reads, by file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SfcFormat {
    /// `.vue`: `<script>` and `<script setup>` blocks.
    Vue,
    /// `.svelte`: the instance `<script>` and the `<script context="module">`
    /// / `<script module>` block.
    Svelte,
    /// `.astro`: the leading `---` frontmatter, then `<script>` blocks.
    Astro,
}

impl SfcFormat {
    /// The format of a file by its extension, `None` for any other file.
    pub fn of_path(path: &str) -> Option<Self> {
        let ext = path.rsplit_once('.')?.1;
        if ext.contains('/') {
            return None;
        }
        match ext {
            "vue" => Some(Self::Vue),
            "svelte" => Some(Self::Svelte),
            "astro" => Some(Self::Astro),
            _ => None,
        }
    }

    /// The format's name, as the `[sfc]` marker counts it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Vue => "vue",
            Self::Svelte => "svelte",
            Self::Astro => "astro",
        }
    }
}

/// [`script_view`]'s answer: the file with only its script text left, and how
/// many script blocks (an Astro frontmatter counts as one) it kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptView {
    /// The source, every byte outside a kept block a space (newlines and
    /// carriage returns kept), so it has the file's length and line breaks.
    pub source: String,
    /// The script blocks kept.
    pub blocks: usize,
    /// The byte offset where the last kept block ends (before its
    /// `</script>`, or Astro's closing `---` line); 0 when none is kept.
    pub end: usize,
}

/// Parse a single-file component (or, for a path no [`SfcFormat`] names,
/// the source as plain TypeScript) through the TypeScript parser.
pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    match SfcFormat::of_path(file_rel_path) {
        Some(format) => {
            let view = script_view(source, format);
            let mut fp = glia_parser_typescript::parse_file(
                &view.source,
                file_rel_path,
                module_qname,
                repo,
            )?;
            restore_module_span(&mut fp, source, &view, module_qname, repo);
            Ok(fp)
        }
        None => glia_parser_typescript::parse_file(source, file_rel_path, module_qname, repo),
    }
}

/// The file MODULE's CODE and POSITION over the file's own text. The
/// TypeScript parser takes a MODULE's cells from its program node, which runs
/// from the first script token to the end of the file, so over a
/// [`script_view`] its CODE holds the blanked markup between and after the
/// blocks as spaces. The MODULE is the script region instead: from that first
/// token to the end of the last block, its CODE the original file's bytes
/// there (any template between two blocks included), a verbatim slice from
/// the POSITION start line that the store keeps as a span (CD.7c), and its
/// POSITION `end_line` that end's row. A single-block file whose block opens
/// the file reads exactly as the pre-FH.6 slice did. A file with no script
/// block (a template-only component) is its markup: from its first
/// non-blank byte to the end of the file, as a TypeScript program's MODULE
/// is. Every other node lies inside one block, where the view and the file
/// agree. Left as parsed when no block holds a token, or the parsed text is
/// not the view's suffix.
fn restore_module_span(
    fp: &mut FileParse,
    source: &str,
    view: &ScriptView,
    module_qname: &str,
    repo: RepoId,
) {
    let module_id =
        glia_core::NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, module_qname);
    let Some(node) = fp.nodes.iter_mut().find(|n| n.id == module_id) else {
        return;
    };
    if view.blocks == 0 {
        let start = source
            .bytes()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(source.len());
        set_span(&mut node.cells, source, start, source.len());
        return;
    }
    let Some(start) = node.cells.iter().find_map(|c| match &c.payload {
        glia_core::CellPayload::Text(text) if c.kind == cell_type::CODE => view
            .source
            .len()
            .checked_sub(text.len())
            .filter(|&start| view.source.get(start..) == Some(text.as_str())),
        _ => None,
    }) else {
        return;
    };
    if start < view.end {
        set_span(&mut node.cells, source, start, view.end);
    }
}

/// Set a MODULE's CODE to `source[start..end]` and its POSITION rows to the
/// rows of `start` and `end`; nothing when the range is no `str` slice.
fn set_span(cells: &mut [glia_core::Cell], source: &str, start: usize, end: usize) {
    let Some(code) = source.get(start..end) else {
        return;
    };
    let row = |at: usize| {
        source.as_bytes()[..at]
            .iter()
            .filter(|&&b| b == b'\n')
            .count()
    };
    let (start_row, end_row) = (row(start), row(end));
    for cell in cells.iter_mut() {
        match &cell.payload {
            glia_core::CellPayload::Text(_) if cell.kind == cell_type::CODE => {
                cell.payload = glia_core::CellPayload::Text(code.to_string());
            }
            glia_core::CellPayload::Json(json) if cell.kind == cell_type::POSITION => {
                if let Some(json) = with_line(json, "start_line", start_row)
                    .and_then(|json| with_line(&json, "end_line", end_row))
                {
                    cell.payload = glia_core::CellPayload::Json(json);
                }
            }
            _ => {}
        }
    }
}

/// A POSITION payload (`{"file":..,"start_line":..,"end_line":..}`) with
/// the integer value of `key` replaced, every other byte kept.
fn with_line(json: &str, key: &str, row: usize) -> Option<String> {
    let needle = format!("\"{key}\":");
    let at = json.find(&needle)? + needle.len();
    let digits = json[at..].bytes().take_while(u8::is_ascii_digit).count();
    (digits > 0).then(|| format!("{}{row}{}", &json[..at], &json[at + digits..]))
}

/// The script text of a single-file component, blanked in place: every
/// `<script ..>` .. `</script>` block whose `type` is JavaScript / TypeScript
/// (none, `module`, `text/javascript`, `ts`, ..; never JSON, an import map or
/// a template), in file order, outside HTML comments, and for Astro the
/// leading `---` frontmatter. Everything else becomes spaces.
pub fn script_view(source: &str, format: SfcFormat) -> ScriptView {
    let bytes = source.as_bytes();
    let mut kept: Vec<(usize, usize)> = Vec::new();
    let mut pos = 0usize;
    if format == SfcFormat::Astro
        && let Some((start, end, after)) = astro_frontmatter(bytes)
    {
        kept.push((start, end));
        pos = after;
    }
    while let Some(off) = bytes
        .get(pos..)
        .and_then(|b| b.iter().position(|&c| c == b'<'))
    {
        let lt = pos + off;
        let rest = &bytes[lt..];
        if rest.starts_with(b"<!--") {
            pos = find(bytes, lt + 4, b"-->").map_or(bytes.len(), |i| i + 3);
            continue;
        }
        if !is_script_open(rest) {
            pos = lt + 1;
            continue;
        }
        let Some(tag_end) = tag_end(bytes, lt + 7) else {
            break;
        };
        let attrs = &source[lt + 7..tag_end];
        if attrs.trim_end().ends_with('/') {
            pos = tag_end + 1;
            continue;
        }
        let content_start = tag_end + 1;
        let close = find_ci(bytes, content_start, b"</script");
        let content_end = close.unwrap_or(bytes.len());
        if is_script_type(attrs) {
            kept.push((content_start, content_end));
        }
        pos = match close {
            Some(c) => bytes[c..]
                .iter()
                .position(|&b| b == b'>')
                .map_or(bytes.len(), |i| c + i + 1),
            None => bytes.len(),
        };
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut next = kept.iter().peekable();
    let mut i = 0usize;
    while i < bytes.len() {
        match next.peek() {
            Some(&&(start, end)) if i >= start => {
                out.extend_from_slice(&bytes[start..end]);
                i = end;
                next.next();
            }
            _ => {
                out.push(match bytes[i] {
                    b @ (b'\n' | b'\r') => b,
                    _ => b' ',
                });
                i += 1;
            }
        }
    }
    // Every kept range starts after an ASCII `>` / line break and ends before
    // an ASCII `<` / line start, and every other byte is ASCII, so the bytes
    // are UTF-8; the lossy arm is unreachable but never panics.
    let source = String::from_utf8(out)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    ScriptView {
        source,
        blocks: kept.len(),
        end: kept.last().map_or(0, |&(_, end)| end),
    }
}

/// The script blocks [`script_view`] keeps in a file, by path; 0 for a path
/// no [`SfcFormat`] names.
pub fn script_blocks(source: &str, path: &str) -> usize {
    SfcFormat::of_path(path).map_or(0, |f| script_view(source, f).blocks)
}

/// Astro's frontmatter: the first non-blank line is `---`, the block runs to
/// the next line that is `---`. `(content start, content end, after the
/// closing fence)`, or `None` with no frontmatter or no closing fence.
fn astro_frontmatter(bytes: &[u8]) -> Option<(usize, usize, usize)> {
    let mut i = 0usize;
    if bytes.starts_with("\u{feff}".as_bytes()) {
        i = 3;
    }
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let (first, after_first) = line_at(bytes, i);
    if first.trim_ascii() != b"---" {
        return None;
    }
    let mut at = after_first;
    while at < bytes.len() {
        let (line, after) = line_at(bytes, at);
        if line.trim_ascii() == b"---" {
            return Some((after_first, at, after));
        }
        at = after;
    }
    None
}

/// The line starting at `start` (without its `\n`) and the offset after it.
fn line_at(bytes: &[u8], start: usize) -> (&[u8], usize) {
    let rest = &bytes[start..];
    match rest.iter().position(|&b| b == b'\n') {
        Some(n) => (&rest[..n], start + n + 1),
        None => (rest, bytes.len()),
    }
}

/// Whether `rest` opens a `<script>` tag (ASCII case-insensitive), the name
/// ending in whitespace, `>` or `/`.
fn is_script_open(rest: &[u8]) -> bool {
    rest.len() > 7
        && rest[..7].eq_ignore_ascii_case(b"<script")
        && matches!(rest[7], b'>' | b'/' | b' ' | b'\t' | b'\n' | b'\r')
}

/// The `>` closing a start tag whose attributes begin at `from`, quoted
/// attribute values stepped over.
fn tag_end(bytes: &[u8], from: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (i, &b) in bytes.iter().enumerate().skip(from) {
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b'>' => return Some(i),
            None => {}
        }
    }
    None
}

/// Whether a script tag's attributes name a JavaScript / TypeScript block:
/// no `type`, or a `type` naming JS / TS (`module`, `text/javascript`,
/// `application/typescript`, `ts`, ..). JSON, import maps and templates are
/// data or markup.
fn is_script_type(attrs: &str) -> bool {
    let Some(value) = attr_value(attrs, "type") else {
        return true;
    };
    let v = value.trim().to_ascii_lowercase();
    matches!(
        v.as_str(),
        "" | "module" | "ts" | "js" | "typescript" | "javascript"
    ) || v.contains("javascript")
        || v.contains("ecmascript")
        || v.contains("typescript")
}

/// The value of attribute `name` in a tag's attribute text, quoted or bare.
fn attr_value<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let b = attrs.as_bytes();
    let mut i = 0usize;
    while i < b.len() {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'=' {
            i += 1;
        }
        let key = &attrs[start..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            if key.is_empty() {
                i += 1;
            }
            continue;
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = match b.get(i) {
            Some(&q @ (b'"' | b'\'')) => {
                let vs = i + 1;
                let ve = b[vs..]
                    .iter()
                    .position(|&c| c == q)
                    .map_or(b.len(), |n| vs + n);
                i = ve + 1;
                &attrs[vs..ve]
            }
            _ => {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                &attrs[vs..i]
            }
        };
        if key.eq_ignore_ascii_case(name) {
            return Some(value);
        }
    }
    None
}

/// The first occurrence of `needle` at or after `from`.
fn find(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| from + i)
}

/// [`find`], ASCII case-insensitive.
fn find_ci(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
        .map(|i| from + i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_core::CellPayload;

    fn repo() -> RepoId {
        RepoId(1)
    }

    /// The 0-based POSITION start row of the node named `name`.
    fn start_row(fp: &FileParse, name: &str) -> u64 {
        let id = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(_, n)| n.as_str() == name)
            .map(|(id, _)| *id)
            .min_by_key(|id| id.0)
            .unwrap_or_else(|| panic!("no node {name}"));
        let node = fp.nodes.iter().find(|n| n.id == id).unwrap();
        let pos = node
            .cells
            .iter()
            .find(|c| c.kind == cell_type::POSITION)
            .unwrap();
        let CellPayload::Json(j) = &pos.payload else {
            panic!("POSITION is JSON")
        };
        let v: serde_json::Value = serde_json::from_str(j).unwrap();
        v["start_line"].as_u64().unwrap()
    }

    #[test]
    fn vue_sfc() {
        let source = r#"
<template>
  <div>{{ message }}</div>
</template>

<script setup lang="ts">
import { ref } from 'vue';

const message = ref('Hello');

function greet() {
  console.log(message.value);
}
</script>
"#;
        let fp = parse_file(source, "src/App.vue", "src::App", repo()).unwrap();
        assert!(fp.nav.name_by_id.values().any(|n| n == "greet"));
        // `function greet()` is the file's line 11 (0-based row 10).
        assert_eq!(start_row(&fp, "greet"), 10);
    }

    /// The MODULE's CODE / POSITION cells.
    fn module_cells(fp: &FileParse, qname: &str) -> (String, String) {
        let module = glia_core::NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, qname);
        let node = fp.nodes.iter().find(|n| n.id == module).unwrap();
        let text = |kind| {
            node.cells
                .iter()
                .find(|c| c.kind == kind)
                .map(|c| match &c.payload {
                    CellPayload::Text(t) | CellPayload::Json(t) => t.clone(),
                    other => panic!("{other:?}"),
                })
                .unwrap()
        };
        (text(cell_type::CODE), text(cell_type::POSITION))
    }

    /// The MODULE is the script region: from the first script token to the
    /// end of the last block, the file's own text (a template between two
    /// blocks included), never the blanked view.
    #[test]
    fn module_spans_the_script_region() {
        let source = "<template>\n  <p>{{ a }}</p>\n</template>\n<script>\nconst a = 1;\n</script>\n\
<p>between</p>\n<script setup>\nconst b = 2;\n</script>\n<style>p {}</style>\n";
        let fp = parse_file(source, "src/A.vue", "src::A", repo()).unwrap();
        let (code, pos) = module_cells(&fp, "src::A");
        assert_eq!(
            code,
            "const a = 1;\n</script>\n<p>between</p>\n<script setup>\nconst b = 2;\n"
        );
        assert_eq!(pos, r#"{"file":"src/A.vue","start_line":4,"end_line":9}"#);
    }

    /// A file whose one block opens it reads as the pre-FH.6 slice did.
    #[test]
    fn module_of_a_leading_block_is_the_block() {
        let source = "<script setup lang=\"ts\">\nconst a = 1;\n</script>\n\n<template>\n  <p/>\n</template>\n";
        let fp = parse_file(source, "src/B.vue", "src::B", repo()).unwrap();
        let (code, pos) = module_cells(&fp, "src::B");
        assert_eq!(code, "const a = 1;\n");
        assert_eq!(pos, r#"{"file":"src/B.vue","start_line":1,"end_line":2}"#);
    }

    /// A template-only component's MODULE is its markup, as the pre-FH.6
    /// whole-file parse located it.
    #[test]
    fn module_of_a_template_only_file_is_its_markup() {
        let source = "<template>\n  <div />\n</template>\n";
        let fp = parse_file(source, "src/Cart.vue", "src::Cart", repo()).unwrap();
        let (code, pos) = module_cells(&fp, "src::Cart");
        assert_eq!(code, source);
        assert_eq!(
            pos,
            r#"{"file":"src/Cart.vue","start_line":0,"end_line":3}"#
        );
    }

    #[test]
    fn end_line_rewrite() {
        assert_eq!(
            with_line(
                r#"{"file":"a","start_line":1,"end_line":17}"#,
                "end_line",
                9
            )
            .as_deref(),
            Some(r#"{"file":"a","start_line":1,"end_line":9}"#)
        );
        assert_eq!(with_line(r#"{"file":"a"}"#, "end_line", 9), None);
    }

    #[test]
    fn plain_ts_fallback() {
        let source = r#"
export function helper() {
  return 42;
}
"#;
        let fp = parse_file(source, "src/util.ts", "src::util", repo()).unwrap();
        assert!(fp.nav.name_by_id.values().any(|n| n == "helper"));
    }

    #[test]
    fn script_view_keeps_lengths_and_lines() {
        let sfc = "<template><div>é</div></template>\n<script>const x = 1;</script>\n";
        let view = script_view(sfc, SfcFormat::Vue);
        assert_eq!(view.blocks, 1);
        assert_eq!(view.source.len(), sfc.len());
        assert_eq!(view.source.lines().count(), sfc.lines().count());
        assert_eq!(view.source.trim(), "const x = 1;");
        let at = sfc.find("const").unwrap();
        assert_eq!(&view.source[at..at + 5], "const");
    }

    #[test]
    fn vue_two_scripts_both_parsed_at_file_lines() {
        let source = "<script lang=\"ts\">\nexport function first() {}\n</script>\n\
<template><p @click=\"second()\">x</p></template>\n\
<script setup lang=\"ts\">\nfunction second() { first(); }\n</script>\n";
        let view = script_view(source, SfcFormat::Vue);
        assert_eq!(view.blocks, 2);
        let fp = parse_file(source, "src/Two.vue", "src::Two", repo()).unwrap();
        assert_eq!(start_row(&fp, "first"), 1);
        assert_eq!(start_row(&fp, "second"), 5);
        let id = |name: &str| {
            fp.nav
                .name_by_id
                .iter()
                .find(|(_, n)| n.as_str() == name)
                .map(|(id, _)| *id)
        };
        let (first, second) = (id("first"), id("second"));
        assert!(
            fp.edges.iter().any(|e| e.category == edge_category::CALLS
                && Some(e.from) == second
                && Some(e.to) == first),
            "the setup block's call binds the other block's function"
        );
    }

    #[test]
    fn svelte_module_and_instance_scripts() {
        let source = "<script context=\"module\">\n  export function preload() {}\n</script>\n\n\
<script lang=\"ts\">\n  import { listAlbums } from './sdk';\n  async function load() {\n    return listAlbums();\n  }\n</script>\n\n\
<h1>{title}</h1>\n<button on:click={load}>x</button>\n";
        let view = script_view(source, SfcFormat::Svelte);
        assert_eq!(view.blocks, 2);
        let fp = parse_file(
            source,
            "src/routes/+page.svelte",
            "src::routes::+page",
            repo(),
        )
        .unwrap();
        assert_eq!(start_row(&fp, "load"), 6);
        assert!(fp.nav.name_by_id.values().any(|n| n == "preload"));
    }

    #[test]
    fn astro_frontmatter_and_script() {
        let source = "---\nimport Layout from '../layouts/Layout.astro';\nfunction title() { return 'x'; }\n---\n\
<Layout><h1>{title()}</h1></Layout>\n<script>\n  function onReady() {}\n</script>\n";
        let view = script_view(source, SfcFormat::Astro);
        assert_eq!(view.blocks, 2);
        assert!(!view.source.contains("---"));
        assert!(!view.source.contains("Layout>"));
        let fp = parse_file(source, "src/pages/index.astro", "src::pages::index", repo()).unwrap();
        assert_eq!(start_row(&fp, "title"), 2);
        assert_eq!(start_row(&fp, "onReady"), 6);
    }

    #[test]
    fn astro_without_frontmatter_reads_scripts_only() {
        let source = "<p>---</p>\n<script>let a = 1;</script>\n";
        let view = script_view(source, SfcFormat::Astro);
        assert_eq!(view.blocks, 1);
        assert_eq!(view.source.trim(), "let a = 1;");
    }

    #[test]
    fn non_script_blocks_skipped() {
        let source = "<!-- <script>let commented = 1;</script> -->\n\
<script type=\"application/ld+json\">{\"a\": 1}</script>\n\
<script type=\"text/x-template\" id=\"t\"><div/></script>\n\
<script src=\"/x.js\" />\n\
<SCRIPT type='module'>let kept = 1;</SCRIPT>\n\
<scripts>not a script</scripts>\n";
        let view = script_view(source, SfcFormat::Vue);
        assert_eq!(view.blocks, 1);
        assert_eq!(view.source.trim(), "let kept = 1;");
    }

    #[test]
    fn template_only_file_has_no_script() {
        let source = "<template>\n  <div />\n</template>\n";
        let view = script_view(source, SfcFormat::Vue);
        assert_eq!(view.blocks, 0);
        assert!(view.source.trim().is_empty());
    }

    #[test]
    fn format_by_extension() {
        assert_eq!(SfcFormat::of_path("a/App.vue"), Some(SfcFormat::Vue));
        assert_eq!(
            SfcFormat::of_path("a/+page.svelte"),
            Some(SfcFormat::Svelte)
        );
        assert_eq!(SfcFormat::of_path("a/index.astro"), Some(SfcFormat::Astro));
        assert_eq!(SfcFormat::of_path("a.vue/x.ts"), None);
        assert_eq!(SfcFormat::of_path("a/util.ts"), None);
        assert_eq!(script_blocks("<script>1</script>", "x.svelte"), 1);
        assert_eq!(script_blocks("<script>1</script>", "x.ts"), 0);
    }

    #[test]
    fn unclosed_script_runs_to_end() {
        let source = "<template/>\n<script>let a = 1;\n";
        let view = script_view(source, SfcFormat::Vue);
        assert_eq!(view.blocks, 1);
        assert_eq!(view.source.trim(), "let a = 1;");
    }
}
