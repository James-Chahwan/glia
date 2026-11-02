use pyo3::prelude::*;

/// The names a change removed, in input order.
pub fn name_breaks(before: &[String], after: &[String]) -> Vec<String> {
    before
        .iter()
        .filter(|name| !after.contains(name))
        .cloned()
        .collect()
}

fn receiver_return(value: u32) -> u32 {
    value + 1
}

fn forbid_edge(from: &str, to: &str) -> bool {
    from != to
}

#[pyfunction]
#[pyo3(name = "do_thing")]
fn do_thing_py() -> u32 {
    if forbid_edge("a", "b") {
        receiver_return(1)
    } else {
        0
    }
}
