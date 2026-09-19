//! Rust → Python value conversion shared by every primitive module.
//!
//! **The return convention (LD.2).** Every pyo3 addition follows it:
//!
//! - An ANSWER returns native Python objects — a `dict`, or a `list` of
//!   `dict` — whose keys are exactly the engine struct's serde fields, in
//!   field order. [`to_py`] is the one conversion.
//! - A method whose NAME ends in `_json` returns a JSON string: the bulk dumps
//!   (`nodes_json`, `edges_json`, `parse_file_to_json`), where `json.loads` on
//!   the caller's side is the fastest path.
//! - Pair-shaped data stays a list of tuples (`activate`, `node_cells`,
//!   `neighbours`, `kind_names`, ...).
//! - A node id is a Python `int` (a `u64`, above `2**63` included), never a
//!   `float`.
//!
//! **Why the answer goes through JSON text.** [`to_py`] takes the engine
//! value's `serde_json` text and decodes it with CPython's `json.loads`. That
//! keeps both halves of the contract without a new dependency: the text lists
//! a struct's fields in declaration order and prints an integer as plain
//! digits (pinned by `answer_values_convert_exactly`), and `json.loads` builds
//! an ordered `dict` and an arbitrary-precision `int` from them. The spec's walk over
//! `serde_json::Value` cannot keep the order here: this workspace builds
//! `serde_json` without `preserve_order`, so `Value`'s map is a `BTreeMap` and
//! every dict would come back with its keys sorted.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

pub(crate) fn escape_json(s: &str) -> String {
    // Delegates to the shared escaper: the four-`replace` version this
    // replaced let every other control character below 0x20 through raw, so a
    // single stray 0x01 in one symbol name or file path made `json.loads`
    // raise `Invalid control character` for the entire graph (audit #16).
    glia_projection_text::escape_json_string(s)
}

/// An engine answer, as native Python objects (the convention above). Pass
/// `serde_json::to_string(&answer)`: a serialisation error becomes a
/// `ValueError`, as the JSON-string returns raised before LD.2.
pub(crate) fn to_py(py: Python<'_>, json: Result<String, serde_json::Error>) -> PyResult<Py<PyAny>> {
    let text = json.map_err(|e| PyValueError::new_err(e.to_string()))?;
    let loads = py.import("json")?.getattr("loads")?;
    Ok(loads.call1((text,))?.unbind())
}

#[cfg(test)]
mod tests {
    /// Does `literal` (one JSON number, as `serde_json` printed it) decode to a
    /// Python `int` rather than a `float`? CPython's `json` reads a number as an
    /// `int` exactly when it has no fraction and no exponent, and an `int` of any
    /// size is exact. So an id survives `to_py` bit for bit iff its text is
    /// plain digits — which `serde_json` guarantees for every `u64` / `i64`, and
    /// which the test below pins for an id above `2**63`, where a detour through
    /// `f64` would round it.
    fn is_exact_int_literal(literal: &str) -> bool {
        let digits = literal.strip_prefix('-').unwrap_or(literal);
        !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
    }

    /// The NodeId the LD.3b probe measured for `users::get_user`: above
    /// `2**63`, so an `f64` detour would change it.
    const BIG_ID: u64 = 13_305_808_056_633_366_085;

    /// LD.2: what `to_py` hands `json.loads` keeps an id above `2**63` as
    /// plain digits (→ Python `int`, exact), a fractional score as a float
    /// literal (→ `float`) and a missing file as `null` (→ `None`), and it
    /// lists a struct's fields in declaration order — the order Python's
    /// `dict` keeps. A real engine record carries the id, so the order and
    /// the digits are the ones a Python caller gets.
    #[test]
    fn answer_values_convert_exactly() {
        let v = serde_json::json!({"id": BIG_ID, "score": 0.25, "file": null});
        let text = serde_json::to_string(&v).expect("serialises");
        let field = |key: &str| -> String {
            let at = text.find(&format!("\"{key}\":")).expect("key present") + key.len() + 3;
            text[at..].split([',', '}']).next().unwrap_or_default().to_string()
        };
        assert_eq!(field("id"), BIG_ID.to_string(), "{text}");
        assert!(is_exact_int_literal(&field("id")), "an id must decode as an int: {text}");
        assert_ne!((BIG_ID as f64) as u64, BIG_ID, "the f64 detour this rules out is lossy");
        assert!(!is_exact_int_literal(&field("score")), "0.25 decodes as a float: {text}");
        assert_eq!(field("score"), "0.25");
        assert_eq!(field("file"), "null");
        assert!(is_exact_int_literal("-42"));
        assert!(!is_exact_int_literal("1e19"));
        assert!(!is_exact_int_literal("1.0"));
        assert!(!is_exact_int_literal(""));

        // A real answer record: fields in the engine struct's order, id digits
        // exact.
        let root = std::env::temp_dir().join(format!("glia-ld2-convert-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp dir");
        std::fs::write(root.join("app.py"), "def helper(x):\n    return x + 1\n").expect("write fixture");
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let merged = built.expect("build").merged;
        let answer = glia_engine::find::find_nodes(
            &merged,
            "helper",
            &glia_engine::find::FindOptions::default(),
        );
        let row = answer.results.first().expect("helper is found");
        let text = serde_json::to_string(row).expect("serialises");
        let at = |key: &str| text.find(&format!("\"{key}\":")).unwrap_or(usize::MAX);
        let order = ["id", "qname", "name", "kind", "file", "line", "match"].map(at);
        assert!(order[0] == 1 && order.windows(2).all(|w| w[0] < w[1]), "{text}");
        let id_text = text["{\"id\":".len()..].split(',').next().unwrap_or_default();
        assert!(is_exact_int_literal(id_text), "{text}");
        assert_eq!(id_text.parse::<u64>().ok(), Some(row.id), "{text}");
    }
}
