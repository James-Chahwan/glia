//! Leading-documentation extraction, shared across all glia language parsers.
//!
//! Every parser is tree-sitter/AST already; this gives them one uniform way to
//! capture the doc comment that precedes a definition — Rust `///` / `//!`, Go
//! Godoc, JSDoc `/** */`, Javadoc, C# `///`, PHPDoc, Swift `///`, Dart `///`,
//! Scala/C/C++ `/* */`, Solidity NatSpec, Ruby `#`. Grammar-agnostic: it keys
//! off node *kind* (`…comment…`) rather than per-language node names, and skips
//! attributes/decorators/annotations sitting between the doc and the item
//! (tree-sitter represents a multi-line `@Component({…})` as ONE node, so the
//! JSDoc above it is still reached — the thing a line-scanner can't do).
//!
//! Body-first-string docstrings (Python, Clojure) are NOT handled here; those
//! parsers extract them from the AST body directly. (glia-v4 D1)

use tree_sitter::Node;

/// Max stored doc length; longer docs are truncated on a char boundary.
pub const DOC_MAX: usize = 500;

/// The canonical POSITION cell payload, shared by every parser so the format
/// and indexing don't drift. JSON `{"file","start_line","end_line"}` with
/// **0-indexed** tree-sitter rows (matching the line-start array the engram
/// exporter indexes for byte spans). Use this for every node's POSITION cell.
pub fn position_json(node: &Node, file_rel: &str) -> String {
    let f = file_rel.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        r#"{{"file":"{}","start_line":{},"end_line":{}}}"#,
        f,
        node.start_position().row,
        node.end_position().row
    )
}

/// Leading doc for `node`, or `None` if there's no preceding comment (or it's
/// boilerplate). `src` is the file's bytes.
pub fn leading_doc(node: &Node, src: &[u8]) -> Option<String> {
    collect_from(node, src).or_else(|| {
        // export/decorated wrappers: the comment sits above the wrapper, not
        // the inner definition. Retry from the parent if this node is its
        // first meaningful child.
        let parent = node.parent()?;
        if parent.start_byte() == node.start_byte()
            || first_named_child_is(&parent, node)
        {
            collect_from(&parent, src)
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
            if let Ok(t) = n.utf8_text(src) {
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
    if blocks.is_empty() {
        return None;
    }
    blocks.reverse();
    clean_and_cap(&blocks.join("\n"))
}

/// Node kinds that can sit between a doc comment and the item it documents:
/// attributes (`#[…]`), decorators/annotations (`@…`), visibility/modifiers.
fn skippable_between(kind: &str) -> bool {
    kind.contains("attribute")
        || kind.contains("decorator")
        || kind.contains("annotation")
        || kind.contains("modifier")
}

/// Strip comment markers line-by-line, drop license/TODO boilerplate, collapse
/// whitespace, cap at [`DOC_MAX`].
fn clean_and_cap(raw: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for line in raw.lines() {
        let s = strip_markers(line.trim());
        if !s.is_empty() {
            out.push(s);
        }
    }
    let joined = out.join(" ");
    let s = joined.trim();
    if s.is_empty() {
        return None;
    }
    let low = s.to_ascii_lowercase();
    if low.contains("copyright")
        || low.contains("spdx-license")
        || low.contains("licensed under")
        || low.contains("all rights reserved")
        || low.contains("permission is hereby granted")
        || low.starts_with("todo")
        || low.starts_with("fixme")
        || low.starts_with("xxx")
        || low.starts_with("hack")
    {
        return None;
    }
    if s.len() <= DOC_MAX {
        return Some(s.to_string());
    }
    let mut end = DOC_MAX;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    Some(s[..end].trim_end().to_string())
}

/// Strip a single line's comment markers (`///` `//!` `//` `/**` `/*` `*/` `*`
/// `///` `#`-with-space and NatSpec `@notice`/`@dev` tags left as text).
fn strip_markers(t: &str) -> String {
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
}
