//! NatSpec tags as engram facts (LG.12).
//!
//! LA.8's Solidity parser writes a DOC_TAGS cell beside the flat DOC string:
//! `{"style":"natspec","tags":[{"tag","name"?,"text"}]}`, tags in source
//! order, untagged text already folded into `notice`, `name` present only when
//! bound (a declared `@param`, a named `@return`, an `@inheritdoc` base), and
//! `text` always present (`""` for a bare `@inheritdoc IVault`). [`tags_of`]
//! reads it and [`map`] turns it into what the exporter emits for the symbol:
//!
//! - `@notice` texts, joined by one space, are the symbol's `doc`; with no
//!   notice the first `@title` is, and emits no fact. Neither leaves the doc
//!   `None`: the flat DOC string is never used for a symbol with a natspec
//!   DOC_TAGS cell, since it repeats every tag.
//! - `@inheritdoc <Base>` names a base (the tag's name, else the first word of
//!   its text) whose same-named function documents this one; no fact.
//! - Every other tag (title, author, dev, param, return, `custom:<id>`, and
//!   anything else LA.8 emits) is one [`TagFact`]: key suffix
//!   `natspec:<tag>[:<name>]`, `:<n>` appended when the symbol already used it
//!   (n = 1, 2, .. in source order), text `<symbol> @<tag>[ <name>]: <text>`.
//!   The symbol name keeps identical tag prose on two functions from sharing
//!   one Engram content key. A tag with neither a name nor text states
//!   nothing and emits no fact.
//!
//! A symbol whose leading comment is plain `//` has DOC and no DOC_TAGS:
//! [`tags_of`] is `None` and the exporter keeps the DOC path for it.

use std::collections::BTreeSet;

use glia_code_domain::cell_type;
use glia_core::{Cell, CellPayload};
use serde_json::Value;

use crate::{cap_doc, clean_and_cap_doc};

/// One tag of a DOC_TAGS cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tag {
    pub(crate) tag: String,
    pub(crate) name: Option<String>,
    pub(crate) text: String,
}

/// LA.8's DOC_TAGS cell ([`cell_type::DOC_TAGS`]) as its tag list, only when
/// its `style` is `"natspec"` and it holds at least one tag. A tag entry
/// without a `tag` string is skipped.
pub(crate) fn tags_of(cells: &[Cell]) -> Option<Vec<Tag>> {
    for c in cells {
        if c.kind != cell_type::DOC_TAGS {
            continue;
        }
        let CellPayload::Json(j) = &c.payload else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(j) else {
            continue;
        };
        if v.get("style").and_then(Value::as_str) != Some("natspec") {
            continue;
        }
        let tags: Vec<Tag> = v
            .get("tags")?
            .as_array()?
            .iter()
            .filter_map(|t| {
                Some(Tag {
                    tag: t.get("tag")?.as_str()?.to_string(),
                    name: t.get("name").and_then(Value::as_str).map(str::to_string),
                    text: t
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                })
            })
            .collect();
        return (!tags.is_empty()).then_some(tags);
    }
    None
}

/// One tag emitted as a Proposition: `<symbol key>#<suffix>` holding `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TagFact {
    pub(crate) suffix: String,
    pub(crate) text: String,
}

/// What one symbol's tags export as.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Mapped {
    /// The symbol's `doc`: the notices, else the first title.
    pub(crate) doc: Option<String>,
    /// Every other tag, in source order.
    pub(crate) facts: Vec<TagFact>,
    /// `@inheritdoc` base names, in source order.
    pub(crate) inheritdoc: Vec<String>,
}

/// Map `symbol_name`'s tags to its doc, tag facts and inheritdoc bases (the
/// rules are in the module docs).
pub(crate) fn map(symbol_name: &str, tags: &[Tag]) -> Mapped {
    let notices: Vec<&str> = tags
        .iter()
        .filter(|t| t.tag == "notice" && !t.text.is_empty())
        .map(|t| t.text.as_str())
        .collect();
    let (doc, title_doc) = if notices.is_empty() {
        let title = tags.iter().position(|t| t.tag == "title");
        (
            title.and_then(|i| clean_and_cap_doc(tags[i].text.clone())),
            title,
        )
    } else {
        (clean_and_cap_doc(notices.join(" ")), None)
    };

    let mut out = Mapped {
        doc,
        ..Mapped::default()
    };
    let mut used: BTreeSet<String> = BTreeSet::new();
    for (i, t) in tags.iter().enumerate() {
        match t.tag.as_str() {
            "notice" => continue,
            "inheritdoc" => {
                let base = t
                    .name
                    .clone()
                    .or_else(|| t.text.split_whitespace().next().map(str::to_string));
                out.inheritdoc.extend(base);
                continue;
            }
            _ if title_doc == Some(i) => continue,
            _ => {}
        }
        if t.name.is_none() && t.text.is_empty() {
            continue;
        }
        let base = match &t.name {
            Some(n) => format!("natspec:{}:{n}", t.tag),
            None => format!("natspec:{}", t.tag),
        };
        let mut suffix = base.clone();
        let mut n = 0usize;
        while used.contains(&suffix) {
            n += 1;
            suffix = format!("{base}:{n}");
        }
        used.insert(suffix.clone());
        let label = match &t.name {
            Some(name) => format!("{symbol_name} @{} {name}", t.tag),
            None => format!("{symbol_name} @{}", t.tag),
        };
        let text = if t.text.is_empty() {
            label
        } else {
            format!("{label}: {}", t.text)
        };
        out.facts.push(TagFact {
            suffix,
            text: cap_doc(&text),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(tag: &str, name: Option<&str>, text: &str) -> Tag {
        Tag {
            tag: tag.into(),
            name: name.map(str::to_string),
            text: text.into(),
        }
    }

    fn fact(suffix: &str, text: &str) -> TagFact {
        TagFact {
            suffix: suffix.into(),
            text: text.into(),
        }
    }

    fn cell(kind: glia_core::CellTypeId, json: &str) -> Cell {
        Cell {
            kind,
            payload: CellPayload::Json(json.into()),
        }
    }

    #[test]
    fn tags_of_reads_the_natspec_list_only() {
        let cells = [cell(
            cell_type::DOC_TAGS,
            r#"{"style":"natspec","tags":[{"tag":"inheritdoc","name":"IVault","text":""},{"tag":"custom:security","text":"non-reentrant"}]}"#,
        )];
        assert_eq!(
            tags_of(&cells),
            Some(vec![
                tag("inheritdoc", Some("IVault"), ""),
                tag("custom:security", None, "non-reentrant")
            ])
        );
        // Another style, an empty list, a malformed cell, or no cell: None.
        assert_eq!(
            tags_of(&[cell(
                cell_type::DOC_TAGS,
                r#"{"style":"jsdoc","tags":[{"tag":"x","text":"y"}]}"#
            )]),
            None
        );
        assert_eq!(
            tags_of(&[cell(
                cell_type::DOC_TAGS,
                r#"{"style":"natspec","tags":[]}"#
            )]),
            None
        );
        assert_eq!(tags_of(&[cell(cell_type::DOC_TAGS, "{")]), None);
        assert_eq!(
            tags_of(&[cell(
                cell_type::ORIGIN,
                r#"{"style":"natspec","tags":[{"tag":"x","text":"y"}]}"#
            )]),
            None
        );
        assert_eq!(tags_of(&[]), None);
    }

    #[test]
    fn notices_join_and_every_other_tag_is_a_fact() {
        let m = map(
            "pay",
            &[
                tag("notice", None, "Pays out."),
                tag("dev", None, "Internal."),
                tag("notice", None, "Twice."),
                tag("param", Some("who"), "The payee."),
                tag("param", None, "amout Misspelt."),
                tag("custom:security", None, "non-reentrant"),
                tag("dev", None, "Again."),
                tag("dev", None, "Thrice."),
            ],
        );
        assert_eq!(m.doc.as_deref(), Some("Pays out. Twice."));
        assert_eq!(
            m.facts,
            vec![
                fact("natspec:dev", "pay @dev: Internal."),
                fact("natspec:param:who", "pay @param who: The payee."),
                fact("natspec:param", "pay @param: amout Misspelt."),
                fact(
                    "natspec:custom:security",
                    "pay @custom:security: non-reentrant"
                ),
                fact("natspec:dev:1", "pay @dev: Again."),
                fact("natspec:dev:2", "pay @dev: Thrice."),
            ]
        );
        assert!(m.inheritdoc.is_empty());
    }

    #[test]
    fn title_is_the_doc_only_without_a_notice() {
        let m = map(
            "Vault",
            &[
                tag("title", None, "A vault"),
                tag("title", None, "Second"),
                tag("author", None, "Kina"),
            ],
        );
        assert_eq!(m.doc.as_deref(), Some("A vault"));
        assert_eq!(
            m.facts,
            vec![
                fact("natspec:title", "Vault @title: Second"),
                fact("natspec:author", "Vault @author: Kina")
            ]
        );
        let m = map(
            "Vault",
            &[tag("title", None, "A vault"), tag("notice", None, "Holds.")],
        );
        assert_eq!(m.doc.as_deref(), Some("Holds."));
        assert_eq!(
            m.facts,
            vec![fact("natspec:title", "Vault @title: A vault")]
        );
    }

    #[test]
    fn inheritdoc_names_a_base_and_empty_tags_state_nothing() {
        let m = map(
            "withdraw",
            &[
                tag("inheritdoc", Some("IVault"), ""),
                tag("inheritdoc", None, "Base extra words"),
                tag("inheritdoc", None, ""),
                tag("dev", None, ""),
                tag("param", Some("amount"), ""),
            ],
        );
        assert_eq!(m.doc, None);
        assert_eq!(m.inheritdoc, ["IVault", "Base"]);
        assert_eq!(
            m.facts,
            vec![fact("natspec:param:amount", "withdraw @param amount")]
        );
    }

    #[test]
    fn fact_text_is_capped_on_a_char_boundary() {
        let long = "é".repeat(400);
        let m = map("f", &[tag("dev", None, &long)]);
        let text = &m.facts[0].text;
        assert!(text.len() <= crate::DOC_MAX, "{}", text.len());
        assert!(text.starts_with("f @dev: é"));
    }
}
