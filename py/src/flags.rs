//! **flags** (CC.7c): pyo3 surface for `glia_engine::flags` (CC.7b) —
//! `PyGraph.flags`, the stale feature-flag report (every flag with its
//! definitions, readers and dead / undefined / single-site / quiet findings)
//! as a native dict (LD.2).

use pyo3::prelude::*;

use glia_engine::flags::{FlagArgs, FlagsReport, flags};
use glia_graph::MergedGraph;

use crate::convert::to_py;
use crate::graph::PyGraph;

// `flags`' `quiet_days=90` below is a literal so `__text_signature__` shows
// it; this keeps it the engine's default.
const _: () = assert!(glia_engine::flags::DEFAULT_QUIET_DAYS == 90);

/// The whole body of [`PyGraph::flags`], minus pyo3 — kept pyo3-free so
/// `cargo test -p glia-py` covers it (see the crate doc). An absence counts
/// the build's unparsed files. Returns the engine report itself, not a
/// `serde_json::Value`: `to_py` decodes its JSON text so the dict keeps the
/// struct's field order (a `Value` map would sort it; `convert.rs`).
fn flag_report(
    merged: &MergedGraph,
    quiet_days: u32,
    scope: Option<String>,
    unparsed_files: usize,
) -> FlagsReport {
    let mut args = FlagArgs::default();
    args.quiet_days = quiet_days;
    args.scope = scope;
    let mut report = flags(merged, &args);
    if let Some(a) = report.absence.as_mut() {
        a.unparsed_files = unparsed_files;
    }
    report
}

#[pymethods]
impl PyGraph {
    /// **flags** (CC.7b / CC.7c): the stale feature-flag report. A flag is
    /// every `config:flag:<key>` CONFIG_KEY (an SDK read: LaunchDarkly,
    /// OpenFeature, Unleash, Flagsmith, Split; a Flipt `flags:` definition),
    /// grouped by key across the merged repos. `quiet_days` is how long every
    /// reader must be unchanged, before the history snapshot's newest change,
    /// for the flag to be quiet; `scope` (a path or project label) keeps a
    /// flag with any site under it, whole.
    ///
    /// Returns a dict `{flags, definitions_in_graph, quiet_evaluated,
    /// history_now, quiet_days, counts, absence}`. Each flag is `{key,
    /// providers, definitions, reads, readers, last_read_change, findings}`:
    /// a site is `{qname, kind, file, line}` (line 1-based, `None` for a
    /// Flipt definition), a finding `{status, tier, note}` with `status` one
    /// of `dead` (derived), `undefined` (derived; only when the graph holds a
    /// flag definition file), `single_site` (fact) or `quiet` (heuristic;
    /// needs `history_sync(..., blame=True)`). Flags with findings sort
    /// first, then by key. `counts` holds every status; `history_now` and
    /// `last_read_change` are unix seconds. `absence` is `None` when any flag
    /// came back, else a dict whose `reason` is `no_match`.
    #[pyo3(signature = (quiet_days=90, scope=None))]
    fn flags(&self, py: Python<'_>, quiet_days: u32, scope: Option<String>) -> PyResult<Py<PyAny>> {
        let report = flag_report(&self.merged, quiet_days, scope, self.parse_errors.len());
        to_py(py, serde_json::to_string(&report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CC.7c: pyo3 `flags` is the engine's `flags` under the given quiet days
    /// and scope, serialised in the engine's field order, the absence
    /// counting unparsed files. The findings themselves are covered by
    /// `engine/tests/flags.rs`.
    #[test]
    fn flags_is_the_engine_flags() {
        let root = std::env::temp_dir().join(format!("glia-cc7c-flags-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (rel, src) in [
            (
                "service/checkout.py",
                "import ldclient\n\nclient = ldclient.get()\n\n\ndef checkout(user):\n    if client.variation(\"new-checkout\", user, False):\n        return 1\n    return 0\n\n\ndef banner(user):\n    return client.variation(\"promo-banner\", user, False)\n",
            ),
            (
                "service/promo.py",
                "import ldclient\n\nclient = ldclient.get()\n\n\ndef show(user):\n    return client.variation(\"promo-banner\", user, False)\n",
            ),
            (
                "flipt/features.yaml",
                "namespace: default\nflags:\n  - key: new-checkout\n    name: New checkout\n  - key: legacy-search\n    name: Legacy search\n",
            ),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("temp dir");
            std::fs::write(path, src).expect("write fixture");
        }
        let built = glia_engine::generate_one(root.to_str().expect("utf-8 temp path"));
        let _ = std::fs::remove_dir_all(&root);
        let built = built.expect("build");

        let report = flag_report(&built.merged, 90, None, 0);
        let keys: Vec<&str> = report.flags.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            ["legacy-search", "new-checkout", "promo-banner"],
            "{report:#?}"
        );
        assert_eq!(report.quiet_days, 90);
        assert!(report.absence.is_none());
        let json = serde_json::to_string(&report).expect("serialises");
        assert!(
            json.starts_with("{\"flags\":[{\"key\":\"legacy-search\",\"providers\":[\"flipt\"]"),
            "{json}"
        );
        assert!(
            json.contains("\"definitions_in_graph\":1,\"quiet_evaluated\":false,\"history_now\":null,\"quiet_days\":90,\"counts\":"),
            "{json}"
        );

        let scoped = flag_report(&built.merged, 30, Some("service/promo.py".into()), 0);
        let keys: Vec<&str> = scoped.flags.iter().map(|f| f.key.as_str()).collect();
        assert_eq!((keys, scoped.quiet_days), (vec!["promo-banner"], 30));

        let none = flag_report(&built.merged, 90, Some("docs".into()), 4);
        let absence = none.absence.expect("an emptied scope is an absence");
        assert_eq!((absence.reason, absence.unparsed_files), ("no_match", 4));
    }
}
