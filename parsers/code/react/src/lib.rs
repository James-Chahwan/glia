//! glia-parser-react — React files through the TypeScript parser.
//!
//! The engine never routes a file to `react`: `detect_language` maps `.tsx`,
//! `.jsx` and `.js` to `typescript`, and the TypeScript parser reads such a
//! file with tree-sitter's TSX grammar (GD.38), so the TypeScript parse IS the
//! React parse. Since GD.39 the TypeScript parser gives components their ROLE
//! from the AST (and reads JSX elements and attributes as USES); the
//! cross-cutting React extractor no longer line-scans component names in
//! TS-family files. [`parse_file`] stays as the
//! `"react"` arm of the engine's explicit-language dispatch.

pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use glia_core::RepoId;

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    glia_parser_typescript::parse_file(source, file_rel_path, module_qname, repo)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    #[test]
    fn react_component() {
        let source = r#"
import React from 'react';

function App() {
  return <div>Hello</div>;
}

export default App;
"#;
        let fp = parse_file(source, "src/App.tsx", "src::App", repo()).unwrap();
        assert!(fp.nav.name_by_id.values().any(|n| n == "App"));
    }
}
