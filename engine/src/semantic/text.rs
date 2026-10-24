//! The embed text recipes (FB.4): what text one node's vector is computed
//! from, and the hash a vector row records of it.
//!
//! Engram measured full text beating name + qname (78 vs 54 of 100); its text
//! is the name, the qname, the leading doc (500-char cap) and up to 10
//! imported libraries not already in the name or qname, never a code body.
//! [`EmbedRecipe::T1`] is that text; [`EmbedRecipe::T2`] appends the node's
//! code LAST, since a model truncates its input (fastembed at 512 tokens) and
//! code must never push the name and doc out. FB.6's gate measures both and
//! picks [`DEFAULT_RECIPE`]. The same functions serve the embed run, the
//! index's freshness check and the gate harness, so the measured text is the
//! shipped text.

use glia_code_domain::cell_type;
use glia_code_domain::profile::EMBED_KINDS;
use glia_core::{CellPayload, Node, NodeId};
use glia_graph::{MergedGraph, RepoGraph};

/// The leading doc's cap, in chars.
pub const DOC_CHARS: usize = 500;
/// How many imported names the text keeps.
pub const MAX_IMPORTS: usize = 10;
/// The code body's cap, in chars ([`EmbedRecipe::T2`] only).
pub const CODE_CHARS: usize = 4000;

/// One way to turn a node into embed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[non_exhaustive]
pub enum EmbedRecipe {
    /// Name, qname, leading doc and imports (Engram's text).
    #[default]
    T1,
    /// [`EmbedRecipe::T1`] plus the node's code, last.
    T2,
}

/// Every recipe, in name order.
pub const RECIPES: [EmbedRecipe; 2] = [EmbedRecipe::T1, EmbedRecipe::T2];

/// The recipe an embed run and a semantic tier use unless told otherwise:
/// T1 until FB.6's gate report picks.
pub const DEFAULT_RECIPE: EmbedRecipe = EmbedRecipe::T1;

impl EmbedRecipe {
    /// `t1` / `t2`: the spelling a CLI flag takes and a `text_hash` starts
    /// with.
    pub fn as_str(self) -> &'static str {
        match self {
            EmbedRecipe::T1 => "t1",
            EmbedRecipe::T2 => "t2",
        }
    }

    /// The recipe [`EmbedRecipe::as_str`] spells, else None.
    pub fn parse(s: &str) -> Option<Self> {
        RECIPES.into_iter().find(|r| r.as_str() == s)
    }
}

impl std::fmt::Display for EmbedRecipe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The text node `id` is embedded from under recipe `r`, read from the first
/// graph holding the id (find's rule). None when no graph holds it, it has no
/// qname, or its kind is not one of [`EMBED_KINDS`].
pub fn embed_text(merged: &MergedGraph, id: NodeId, r: EmbedRecipe) -> Option<String> {
    merged.graphs.iter().find_map(|g| {
        if !g.nav.kind_by_id.contains_key(&id) {
            return None;
        }
        let n = g.nodes.iter().find(|n| n.id == id)?;
        node_text(g, n, r)
    })
}

/// `<recipe>:<first 16 lowercase hex of blake3(text)>`, the form a vector
/// row's `text_hash` records (`external_inputs::text_hash_ok`).
pub fn text_hash(r: EmbedRecipe, text: &str) -> String {
    let hex = blake3::hash(text.as_bytes()).to_hex();
    format!("{}:{}", r.as_str(), &hex.as_str()[..16])
}

/// [`embed_text`] for a node already found in graph `g`.
pub(crate) fn node_text(g: &RepoGraph, n: &Node, r: EmbedRecipe) -> Option<String> {
    let kind = g.nav.kind_by_id.get(&n.id)?;
    if !EMBED_KINDS.contains(kind) {
        return None;
    }
    let qname = g.nav.qname_by_id.get(&n.id)?;
    let name = g
        .nav
        .name_by_id
        .get(&n.id)
        .map(String::as_str)
        .unwrap_or("");
    let mut parts: Vec<String> = Vec::with_capacity(5);
    for p in [name, qname.as_str()] {
        if !p.trim().is_empty() {
            parts.push(p.to_string());
        }
    }
    if let Some(doc) = first_text(n, cell_type::DOC) {
        let doc = cut(&collapse_ws(doc), DOC_CHARS);
        if !doc.is_empty() {
            parts.push(doc);
        }
    }
    let imports = imports(n, name, qname);
    if !imports.is_empty() {
        parts.push(imports.join(" "));
    }
    if r == EmbedRecipe::T2
        && let Some(code) = first_text(n, cell_type::CODE)
    {
        let code = cut(code, CODE_CHARS);
        if !code.trim().is_empty() {
            parts.push(code);
        }
    }
    Some(parts.join("\n"))
}

/// The first cell of `kind` whose payload is text. A CODE span that did not
/// resolve on load is JSON (`{"code_span":..}`), so it counts as no code.
fn first_text(n: &Node, kind: glia_core::CellTypeId) -> Option<&str> {
    n.cells.iter().find_map(|c| match &c.payload {
        CellPayload::Text(s) if c.kind == kind => Some(s.as_str()),
        _ => None,
    })
}

/// The IMPORTS cell's names in stored order, dropping any the name or qname
/// already contains (ASCII case ignored), at most [`MAX_IMPORTS`].
fn imports(n: &Node, name: &str, qname: &str) -> Vec<String> {
    let Some(raw) = n.cells.iter().find_map(|c| match &c.payload {
        CellPayload::Json(s) | CellPayload::Text(s) if c.kind == cell_type::IMPORTS => {
            Some(s.as_str())
        }
        _ => None,
    }) else {
        return Vec::new();
    };
    let Ok(names) = serde_json::from_str::<Vec<String>>(raw) else {
        return Vec::new();
    };
    let name_lc = name.to_ascii_lowercase();
    let qname_lc = qname.to_ascii_lowercase();
    names
        .into_iter()
        .filter(|imp| {
            let lc = imp.trim().to_ascii_lowercase();
            !lc.is_empty() && !name_lc.contains(&lc) && !qname_lc.contains(&lc)
        })
        .take(MAX_IMPORTS)
        .collect()
}

/// Runs of whitespace as one space, trimmed.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first `max` chars of `s` (a char boundary by construction).
fn cut(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => s[..at].to_string(),
        None => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipes_round_trip_and_hash_form() {
        for r in RECIPES {
            assert_eq!(EmbedRecipe::parse(r.as_str()), Some(r));
        }
        assert_eq!(EmbedRecipe::parse("t3"), None);
        let h = text_hash(EmbedRecipe::T2, "abc");
        assert!(glia_code_domain::external_inputs::text_hash_ok(&h), "{h}");
        assert!(h.starts_with("t2:"));
        assert_ne!(text_hash(EmbedRecipe::T1, "abc"), h);
    }

    #[test]
    fn cut_respects_char_boundaries() {
        assert_eq!(cut("héllo", 2), "hé");
        assert_eq!(cut("ab", 5), "ab");
        assert_eq!(collapse_ws("  a \n\t b "), "a b");
    }
}
