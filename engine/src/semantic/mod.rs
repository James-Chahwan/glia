//! Query-time embeddings (0.5.4, FB): the text recipe each node is embedded
//! from, the vector index over `.glia/vectors.jsonl`, the `glia embed` run that
//! writes it (a snapshot step, never a build pass) and the semantic tier an
//! answer may add. A semantic row ranks or annotates and never decides presence
//! (seed §B): it never promotes, hides or creates a FACT / DERIVED row, never
//! seeds a walk, and always sits beside the lexical absence. FB.4 owns this file
//! and adds its child `mod` lines and re-exports here (+FB.5a, FB.6, FB.7a, FB.8).

mod index;
mod text;

pub use index::{IndexStats, SemanticHit, VECTORS_FILE, VectorIndex};
pub use text::{
    CODE_CHARS, DEFAULT_RECIPE, DOC_CHARS, EmbedRecipe, MAX_IMPORTS, RECIPES, embed_text, text_hash,
};
