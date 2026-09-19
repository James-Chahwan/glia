//! Leading-documentation extraction, shared across all glia language parsers.
//!
//! Every parser is tree-sitter/AST already; this gives them one uniform way to
//! capture the doc comment that precedes a definition — Rust `///` / `//!`, Go
//! Godoc, JSDoc `/** */`, Javadoc, C# `///`, PHPDoc, Swift `///`, Dart `///`,
//! Scala/C/C++ `/* */`, Solidity NatSpec, Ruby `#`, Clojure `;`/`;;`. Grammar-agnostic: it keys
//! off node *kind* (`…comment…`) rather than per-language node names, and skips
//! attributes/decorators/annotations sitting between the doc and the item
//! (tree-sitter represents a multi-line `@Component({…})` as ONE node, so the
//! JSDoc above it is still reached — the thing a line-scanner can't do).
//!
//! Body-first-string docstrings (Python, Clojure) are NOT handled here; those
//! parsers extract them from the AST body directly. (glia-v4 D1)
//!
//! [`leading_doc`] is the flat prose string (the DOC cell). [`leading_doc_lines`]
//! is the same doc before its lines are joined, and [`split_doc_tags`] turns
//! those lines into structured `@tag`s — Solidity NatSpec's DOC_TAGS cell is the
//! first user (LA.8).

use tree_sitter::Node;

/// Max stored doc length; longer docs are truncated on a char boundary.
pub const DOC_MAX: usize = 500;

/// The canonical POSITION cell payload, shared by every parser so the format
/// and indexing don't drift. JSON `{"file","start_line","end_line"}` with
/// **0-indexed** tree-sitter rows (matching the line-start array the engram
/// exporter indexes for byte spans). Use this for every node's POSITION cell.
pub fn position_json(node: &Node, file_rel: &str) -> String {
    position_json_span(node, node, file_rel)
}

/// [`position_json`] for an entity that spans several sibling nodes: the start
/// row of `first`, the end row of `last`, same JSON and escaping. For grammars
/// that put a declaration's body BESIDE its signature (tree-sitter-dart's
/// top-level `function_signature` + `function_body`, LA.37a), so the POSITION
/// covers both.
pub fn position_json_span(first: &Node, last: &Node, file_rel: &str) -> String {
    let f = file_rel.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"{{"file":"{}","start_line":{},"end_line":{}}}"#,
        f,
        first.start_position().row,
        last.end_position().row
    )
}

/// Leading doc for `node`, or `None` if there's no preceding comment (or it's
/// boilerplate). `src` is the file's bytes.
pub fn leading_doc(node: &Node, src: &[u8]) -> Option<String> {
    with_parent_fallback(node, src, collect_from)
}

/// The doc [`leading_doc`] returns, as its marker-stripped lines BEFORE they are
/// joined — same sibling walk, same parent fallback, same boilerplate filter, so
/// this is `None` exactly when [`leading_doc`] is. The per-line structure is
/// what doc-tag grammars need ([`split_doc_tags`]); the joined string has lost
/// it. Lines are not capped: cap what you derive from them.
pub fn leading_doc_lines(node: &Node, src: &[u8]) -> Option<Vec<String>> {
    with_parent_fallback(node, src, |n, s| collect_lines(n, s, |_| true))
}

/// [`leading_doc_lines`] restricted to the comment blocks `keep` accepts (it
/// sees each comment node's raw text, markers included). A rejected block is
/// stepped over like an attribute rather than ending the walk, so a plain
/// `// section` comment neither contributes lines nor hides the doc block
/// above it. For doc dialects that only some comment forms carry — Solidity
/// NatSpec is `///` / `/** */`, never `//` / `/* */`.
pub fn leading_doc_lines_where(
    node: &Node,
    src: &[u8],
    keep: fn(&str) -> bool,
) -> Option<Vec<String>> {
    with_parent_fallback(node, src, |n, s| collect_lines(n, s, keep))
}

/// Run `collect` on `node`; failing that — export/decorated wrappers, where the
/// comment sits above the wrapper rather than the inner definition — on its
/// parent when `node` is the parent's first meaningful child.
fn with_parent_fallback<T>(
    node: &Node,
    src: &[u8],
    collect: impl Fn(&Node, &[u8]) -> Option<T>,
) -> Option<T> {
    collect(node, src).or_else(|| {
        let parent = node.parent()?;
        if parent.start_byte() == node.start_byte()
            || first_named_child_is(&parent, node)
        {
            collect(&parent, src)
        } else {
            None
        }
    })
}

fn first_named_child_is(parent: &Node, node: &Node) -> bool {
    let mut cur = parent.walk();
    parent
        .named_children(&mut cur)
        .next()
        .is_some_and(|c| c.id() == node.id())
}

fn collect_from(node: &Node, src: &[u8]) -> Option<String> {
    collect_lines(node, src, |_| true).map(|lines| cap_doc(&lines.join(" ")))
}

/// The comment blocks directly above `node` that `keep` accepts, in source
/// order, verbatim (markers included). Attributes / decorators / annotations /
/// modifiers and rejected comments are stepped over; anything else ends the
/// walk.
fn raw_blocks(node: &Node, src: &[u8], keep: impl Fn(&str) -> bool) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut cur = node.prev_sibling();
    let mut hops = 0u32;
    while let Some(n) = cur {
        hops += 1;
        if hops > 64 {
            break; // safety against pathological trees
        }
        let kind = n.kind();
        if kind.contains("comment") {
            if let Ok(t) = n.utf8_text(src)
                && keep(t)
            {
                blocks.push(t.to_string());
            }
            cur = n.prev_sibling();
            continue;
        }
        if skippable_between(kind) {
            cur = n.prev_sibling();
            continue;
        }
        break;
    }
    blocks.reverse();
    blocks
}

fn collect_lines(node: &Node, src: &[u8], keep: impl Fn(&str) -> bool) -> Option<Vec<String>> {
    let blocks = raw_blocks(node, src, keep);
    if blocks.is_empty() {
        return None;
    }
    clean_lines(&blocks.join("\n"))
}

/// Node kinds that can sit between a doc comment and the item it documents:
/// attributes (`#[…]`), decorators/annotations (`@…`), visibility/modifiers.
fn skippable_between(kind: &str) -> bool {
    kind.contains("attribute")
        || kind.contains("decorator")
        || kind.contains("annotation")
        || kind.contains("modifier")
}

/// Strip comment markers line-by-line and drop blank lines. `None` when nothing
/// is left or the text is license / TODO boilerplate.
fn clean_lines(raw: &str) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for line in raw.lines() {
        let s = strip_markers(line.trim());
        if !s.is_empty() {
            out.push(s);
        }
    }
    let joined = out.join(" ");
    let s = joined.trim();
    if s.is_empty() || is_boilerplate(s) {
        return None;
    }
    Some(out)
}

fn is_boilerplate(s: &str) -> bool {
    let low = s.to_ascii_lowercase();
    low.contains("copyright")
        || low.contains("spdx-license")
        || low.contains("licensed under")
        || low.contains("all rights reserved")
        || low.contains("permission is hereby granted")
        || low.starts_with("todo")
        || low.starts_with("fixme")
        || low.starts_with("xxx")
        || low.starts_with("hack")
}

/// Trim, then cap at [`DOC_MAX`] bytes on a char boundary.
fn cap_doc(s: &str) -> String {
    let s = s.trim();
    if s.len() <= DOC_MAX {
        return s.to_string();
    }
    let mut end = DOC_MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].trim_end().to_string()
}

/// Tags whose first word names something: a parameter, a return variable, the
/// base a doc is inherited from.
const NAMED_TAGS: &[&str] = &["param", "return", "inheritdoc"];

/// One structured doc tag, e.g. `@param amount The amount in wei.`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocTag {
    /// The tag without its `@`: `notice`, `param`, `custom:security`.
    pub tag: String,
    /// For `param` / `return` / `inheritdoc`, the first word of the tag's text
    /// as a NAME candidate. The grammar only proposes it; the language parser
    /// checks it against the AST and calls [`DocTag::unbind_name`] when it does
    /// not name anything real.
    pub name: Option<String>,
    /// Whitespace-collapsed description (without the name), capped at
    /// [`DOC_MAX`].
    pub text: String,
}

impl DocTag {
    /// Reject the NAME candidate: it goes back to being the first word of the
    /// text, so a misspelt `@param amout …` stays prose instead of inventing a
    /// parameter.
    pub fn unbind_name(&mut self) {
        if let Some(name) = self.name.take() {
            let joined = if self.text.is_empty() {
                name
            } else {
                format!("{name} {}", self.text)
            };
            self.text = cap_doc(&joined);
        }
    }
}

/// Split marker-stripped doc lines (from [`leading_doc_lines`]) into `@tag`s.
///
/// Grammar, line by line in source order: a line starting `@<tag>` opens a tag,
/// where `<tag>` is `[A-Za-z][A-Za-z0-9_-]*` or `custom:` + the same; the rest
/// of the line starts its text. Any other line — including one starting with
/// an `@` that is not a valid tag — continues the open tag's text, or opens
/// `default_tag` when no tag is open yet (NatSpec: untagged text is `@notice`).
/// An `@` anywhere but the start of a line is text. For [`NAMED_TAGS`] the first
/// word of the collected text becomes [`DocTag::name`]. Tags keep source order.
pub fn split_doc_tags(lines: &[String], default_tag: &str) -> Vec<DocTag> {
    let mut open: Vec<(String, String)> = Vec::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((tag, rest)) = tag_line(line) {
            open.push((tag.to_string(), rest.to_string()));
        } else if let Some((_, text)) = open.last_mut() {
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(line);
        } else {
            open.push((default_tag.to_string(), line.to_string()));
        }
    }
    open.into_iter()
        .map(|(tag, raw)| finish_tag(tag, &raw))
        .collect()
}

/// `@tag rest…` → `(tag, rest)`; `None` when the line does not open a tag.
fn tag_line(line: &str) -> Option<(&str, &str)> {
    let body = line.strip_prefix('@')?;
    let end = body.find(char::is_whitespace).unwrap_or(body.len());
    let (tag, rest) = body.split_at(end);
    is_tag_name(tag).then_some((tag, rest.trim()))
}

fn is_tag_name(tag: &str) -> bool {
    fn ident(s: &str) -> bool {
        let mut cs = s.chars();
        cs.next().is_some_and(|c| c.is_ascii_alphabetic())
            && cs.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }
    match tag.strip_prefix("custom:") {
        Some(id) => ident(id),
        None => ident(tag),
    }
}

fn finish_tag(tag: String, raw: &str) -> DocTag {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let (name, text) = if NAMED_TAGS.contains(&tag.as_str()) {
        match collapsed.split_once(' ') {
            Some((name, rest)) => (Some(cap_doc(name)), rest.to_string()),
            None if collapsed.is_empty() => (None, String::new()),
            None => (Some(cap_doc(&collapsed)), String::new()),
        }
    } else {
        (None, collapsed)
    };
    DocTag {
        tag,
        name,
        text: cap_doc(&text),
    }
}

/// Strip a single line's comment markers (`///` `//!` `//` `/**` `/*` `*/` `*`
/// `///` `#`-with-space and NatSpec `@notice`/`@dev` tags left as text).
///
/// A line that STARTS with `;` is a Lisp/Clojure comment (`;`, `;;`, `;;;`):
/// the whole run of `;` is its one marker, so the text after it is returned
/// as-is — `;; * note` keeps its `*`. No other language's doc comment starts
/// with `;`, so every other parser sees the old behaviour. (LA.7b)
fn strip_markers(t: &str) -> String {
    if t.starts_with(';') {
        return t.trim_start_matches(';').trim().to_string();
    }
    let mut s = t;
    for m in ["///", "//!", "//", "/**", "/*", "*/", "*"] {
        if let Some(rest) = s.strip_prefix(m) {
            s = rest;
            break;
        }
    }
    // Ruby/shell `#` doc lines (but not `#!`/`#[` handled by skippable_between).
    if let Some(rest) = s.strip_prefix("# ") {
        s = rest;
    }
    s.trim().trim_end_matches("*/").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tree_sitter::Parser;

    fn rust_tree(src: &str) -> tree_sitter::Tree {
        let mut p = Parser::new();
        p.set_language(&tree_sitter_rust::LANGUAGE.into()).unwrap();
        p.parse(src, None).unwrap()
    }

    /// Find the first node of `kind` in the tree (DFS).
    fn find<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        if node.kind() == kind {
            return Some(node);
        }
        let mut cur = node.walk();
        for child in node.children(&mut cur) {
            if let Some(n) = find(child, kind) {
                return Some(n);
            }
        }
        None
    }

    #[test]
    fn rust_doc_comment_above_fn() {
        let src = "/// Hashes a password securely.\npub fn hash() {}";
        let tree = rust_tree(src);
        let f = find(tree.root_node(), "function_item").unwrap();
        assert_eq!(
            leading_doc(&f, src.as_bytes()).as_deref(),
            Some("Hashes a password securely.")
        );
    }

    #[test]
    fn rust_doc_above_attribute() {
        // The `#[inline]` attribute sits between the doc and the fn — skipped.
        let src = "/// Adds two numbers.\n#[inline]\npub fn add() {}";
        let tree = rust_tree(src);
        let f = find(tree.root_node(), "function_item").unwrap();
        assert_eq!(leading_doc(&f, src.as_bytes()).as_deref(), Some("Adds two numbers."));
    }

    #[test]
    fn block_doc_multiline() {
        let src = "/**\n * Sends the email.\n * @param to recipient\n */\npub fn send() {}";
        let tree = rust_tree(src);
        let f = find(tree.root_node(), "function_item").unwrap();
        assert_eq!(
            leading_doc(&f, src.as_bytes()).as_deref(),
            Some("Sends the email. @param to recipient")
        );
    }

    #[test]
    fn position_json_is_json_zero_indexed() {
        let src = "fn a() {}\nfn b() {}";
        let tree = rust_tree(src);
        let b = {
            // second function, starts on row 1 (0-indexed).
            let mut cur = tree.root_node().walk();
            tree.root_node()
                .children(&mut cur)
                .filter(|n| n.kind() == "function_item")
                .nth(1)
                .unwrap()
        };
        assert_eq!(
            position_json(&b, "src/x.rs"),
            r#"{"file":"src/x.rs","start_line":1,"end_line":1}"#
        );
    }

    /// LA.37a: a span over two sibling nodes starts at the first's row and
    /// ends at the second's; the file name is escaped as in `position_json`.
    #[test]
    fn position_json_span_covers_both_nodes() {
        let src = "fn a() {}\n\nfn b() {\n}\n";
        let tree = rust_tree(src);
        let fns: Vec<Node> = {
            let mut cur = tree.root_node().walk();
            tree.root_node()
                .children(&mut cur)
                .filter(|n| n.kind() == "function_item")
                .collect()
        };
        assert_eq!(
            position_json_span(&fns[0], &fns[1], "src/\"x\".rs"),
            r#"{"file":"src/\"x\".rs","start_line":0,"end_line":3}"#
        );
        assert_eq!(
            position_json_span(&fns[1], &fns[1], "src/x.rs"),
            position_json(&fns[1], "src/x.rs")
        );
    }

    #[test]
    fn no_doc_returns_none() {
        let src = "pub fn bare() {}";
        let tree = rust_tree(src);
        let f = find(tree.root_node(), "function_item").unwrap();
        assert_eq!(leading_doc(&f, src.as_bytes()), None);
    }

    #[test]
    fn license_header_skipped() {
        let src = "// Copyright 2026 Acme. All rights reserved.\npub fn f() {}";
        let tree = rust_tree(src);
        let f = find(tree.root_node(), "function_item").unwrap();
        assert_eq!(leading_doc(&f, src.as_bytes()), None);
    }

    fn lines(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    fn tag(tag: &str, name: Option<&str>, text: &str) -> DocTag {
        DocTag {
            tag: tag.into(),
            name: name.map(Into::into),
            text: text.into(),
        }
    }

    #[test]
    fn leading_doc_lines_is_leading_doc_before_the_join() {
        for src in [
            "/// Hashes a password securely.\npub fn hash() {}",
            "/// Adds two numbers.\n#[inline]\npub fn add() {}",
            "/**\n * Sends the email.\n * @param to recipient\n */\npub fn send() {}",
            "pub fn bare() {}",
            "// Copyright 2026 Acme. All rights reserved.\npub fn f() {}",
        ] {
            let tree = rust_tree(src);
            let f = find(tree.root_node(), "function_item").unwrap();
            let joined = leading_doc_lines(&f, src.as_bytes()).map(|l| l.join(" "));
            assert_eq!(joined, leading_doc(&f, src.as_bytes()), "{src}");
        }
        let src = "/**\n * Sends the email.\n *\n * @param to recipient\n */\npub fn send() {}";
        let tree = rust_tree(src);
        let f = find(tree.root_node(), "function_item").unwrap();
        assert_eq!(
            leading_doc_lines(&f, src.as_bytes()),
            Some(lines(&["Sends the email.", "@param to recipient"]))
        );
    }

    #[test]
    fn leading_doc_lines_where_steps_over_rejected_blocks() {
        let src = "/// @notice Kept.\n// section divider\npub fn f() {}";
        let tree = rust_tree(src);
        let f = find(tree.root_node(), "function_item").unwrap();
        let only_triple = |t: &str| t.starts_with("///");
        assert_eq!(
            leading_doc_lines_where(&f, src.as_bytes(), only_triple),
            Some(lines(&["@notice Kept."]))
        );
        // Nothing accepted -> None, while the unfiltered walk sees both.
        let src = "// just a comment\npub fn g() {}";
        let tree = rust_tree(src);
        let g = find(tree.root_node(), "function_item").unwrap();
        assert_eq!(leading_doc_lines_where(&g, src.as_bytes(), only_triple), None);
        assert_eq!(
            leading_doc_lines(&g, src.as_bytes()),
            Some(lines(&["just a comment"]))
        );
    }

    #[test]
    fn split_doc_tags_multiline_continuation() {
        let got = split_doc_tags(
            &lines(&[
                "@dev Returns the symbol of the token, usually a shorter version of the",
                "name.",
                "@param amount   The amount",
                "in   wei.",
            ]),
            "notice",
        );
        assert_eq!(
            got,
            vec![
                tag(
                    "dev",
                    None,
                    "Returns the symbol of the token, usually a shorter version of the name."
                ),
                tag("param", Some("amount"), "The amount in wei."),
            ]
        );
    }

    #[test]
    fn split_doc_tags_untagged_text_opens_the_default_tag() {
        let got = split_doc_tags(&lines(&["Deposits funds.", "@dev Internal."]), "notice");
        assert_eq!(
            got,
            vec![tag("notice", None, "Deposits funds."), tag("dev", None, "Internal.")]
        );
    }

    #[test]
    fn split_doc_tags_custom_tag() {
        let got = split_doc_tags(&lines(&["@custom:security non-reentrant"]), "notice");
        assert_eq!(got, vec![tag("custom:security", None, "non-reentrant")]);
        // `custom:` with no id is not a tag: the line is default-tag text.
        let got = split_doc_tags(&lines(&["@custom: nope"]), "notice");
        assert_eq!(got, vec![tag("notice", None, "@custom: nope")]);
    }

    #[test]
    fn split_doc_tags_at_inside_text_stays_text() {
        let got = split_doc_tags(
            &lines(&[
                "@notice Mail admin@example.com or @ops for help.",
                "@2x assets are served separately.",
            ]),
            "notice",
        );
        assert_eq!(
            got,
            vec![tag(
                "notice",
                None,
                "Mail admin@example.com or @ops for help. @2x assets are served separately."
            )]
        );
    }

    #[test]
    fn split_doc_tags_named_tags_propose_a_name() {
        let got = split_doc_tags(
            &lines(&["@return True always.", "@inheritdoc IVault", "@param"]),
            "notice",
        );
        assert_eq!(
            got,
            vec![
                tag("return", Some("True"), "always."),
                tag("inheritdoc", Some("IVault"), ""),
                tag("param", None, ""),
            ]
        );
        let mut rejected = got[0].clone();
        rejected.unbind_name();
        assert_eq!(rejected, tag("return", None, "True always."));
        let mut bare = got[1].clone();
        bare.unbind_name();
        assert_eq!(bare, tag("inheritdoc", None, "IVault"));
    }

    #[test]
    fn clojure_semicolon_comment_markers_are_stripped() {
        assert_eq!(strip_markers(";; leading comment"), "leading comment");
        assert_eq!(strip_markers("; one"), "one");
        assert_eq!(strip_markers(";;;section"), "section");
        // The `;` run is the only marker: what follows it is text.
        assert_eq!(strip_markers(";; * bullet"), "* bullet");
        // Only a line that STARTS with `;` is touched.
        assert_eq!(strip_markers("// a; b"), "a; b");
        assert_eq!(strip_markers("x ;; y"), "x ;; y");
        // Through the shared line cleaner: `;;` lines, blank `;;` lines dropped.
        assert_eq!(
            clean_lines(";; Line one.\n;;\n;; Line two."),
            Some(lines(&["Line one.", "Line two."]))
        );
    }

    #[test]
    fn split_doc_tags_caps_text_on_a_char_boundary() {
        let long = format!("@dev {}", "é".repeat(DOC_MAX));
        let got = split_doc_tags(&[long], "notice");
        assert!(got[0].text.len() <= DOC_MAX);
        assert!(got[0].text.chars().all(|c| c == 'é'));
    }
}
