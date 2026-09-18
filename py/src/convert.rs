//! Rust → Python value conversion shared by every primitive module.

pub(crate) fn escape_json(s: &str) -> String {
    // Delegates to the shared escaper: the four-`replace` version this
    // replaced let every other control character below 0x20 through raw, so a
    // single stray 0x01 in one symbol name or file path made `json.loads`
    // raise `Invalid control character` for the entire graph (audit #16).
    repo_graph_projection_text::escape_json_string(s)
}
