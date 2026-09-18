//! Code-domain activation defaults and the task-tuned weight profiles.

use std::collections::HashMap;

use repo_graph_code_domain::edge_category;

// ============================================================================
// Code-domain activation defaults
// ============================================================================

/// Default `ActivationConfig` for code graphs. Weights: `calls` and
/// `http_calls` highest, `imports` medium, structural edges (`contains`,
/// `defines`) lowest. Direction forward (impact analysis default).
pub fn code_activation_defaults() -> repo_graph_activation::ActivationConfig {
    use repo_graph_activation::{ActivationConfig, Direction, Specificity};

    let mut weights = HashMap::new();
    weights.insert(edge_category::CALLS, 5.0);
    weights.insert(edge_category::HTTP_CALLS, 5.0);
    weights.insert(edge_category::GRPC_CALLS, 5.0);
    weights.insert(edge_category::RPC_CALLS, 5.0);
    weights.insert(edge_category::GRAPHQL_CALLS, 5.0);
    weights.insert(edge_category::QUEUE_FLOWS, 4.0);
    weights.insert(edge_category::WS_CONNECTS, 4.0);
    weights.insert(edge_category::EVENT_FLOWS, 4.0);
    weights.insert(edge_category::CLI_INVOKES, 3.0);
    weights.insert(edge_category::NAVIGATES_TO, 3.0);
    weights.insert(edge_category::HANDLED_BY, 4.0);
    weights.insert(edge_category::IMPORTS, 3.0);
    weights.insert(edge_category::USES, 3.0);
    weights.insert(edge_category::SHARES_SCHEMA, 2.0);
    weights.insert(edge_category::TESTS, 2.0);
    weights.insert(edge_category::INJECTS, 2.0);
    weights.insert(edge_category::DEFINES, 1.0);
    weights.insert(edge_category::CONTAINS, 1.0);
    weights.insert(edge_category::DOCUMENTS, 0.5);

    ActivationConfig {
        damping: 0.5,
        direction: Direction::Forward,
        edge_weights: weights,
        node_specificity: Specificity::None,
        top_k: 50,
        max_iterations: 100,
        epsilon: 1e-6,
    }
}

/// Task-tuned edge-weight presets over [`code_activation_defaults`] (WP-F /
/// GR-5). Same PPR engine, different lens on which relationships matter:
/// - `"repair"` upweights what buggy code actually touches (calls + data/config
///   access + tests).
/// - `"review"` upweights structural relationships a reviewer reasons over
///   (calls, tests, implements/inherits, return types).
/// - `"onboard"` upweights the high-level shape (entry points + module /
///   containment / docs).
/// - `"default"` (or any unknown profile) returns the defaults unchanged.
pub fn code_activation_profile(profile: &str) -> repo_graph_activation::ActivationConfig {
    let mut config = code_activation_defaults();
    let w = &mut config.edge_weights;
    match profile {
        "repair" => {
            w.insert(edge_category::CALLS, 8.0);
            w.insert(edge_category::USES, 6.0);
            w.insert(edge_category::ACCESSES_DATA, 6.0);
            w.insert(edge_category::READS_CONFIG, 5.0);
            w.insert(edge_category::IMPORTS, 5.0);
            w.insert(edge_category::TESTS, 4.0);
        }
        "review" => {
            w.insert(edge_category::CALLS, 6.0);
            w.insert(edge_category::TESTS, 6.0);
            w.insert(edge_category::IMPLEMENTS, 5.0);
            w.insert(edge_category::INHERITS_FROM, 5.0);
            w.insert(edge_category::RETURNS_TYPE, 4.0);
        }
        "onboard" => {
            w.insert(edge_category::CONTAINS, 5.0);
            w.insert(edge_category::IMPORTS, 5.0);
            w.insert(edge_category::HANDLED_BY, 6.0);
            w.insert(edge_category::DEFINES, 3.0);
            w.insert(edge_category::DOCUMENTS, 3.0);
        }
        _ => {}
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::edge_category;

    #[test]
    fn activation_profiles_shift_weights() {
        let def = code_activation_defaults();
        let repair = code_activation_profile("repair");
        let onboard = code_activation_profile("onboard");
        // repair upweights CALLS above the default.
        assert!(
            repair.edge_weights[&edge_category::CALLS]
                > def.edge_weights[&edge_category::CALLS]
        );
        // onboard upweights CONTAINS above the default.
        assert!(
            onboard.edge_weights[&edge_category::CONTAINS]
                > def.edge_weights[&edge_category::CONTAINS]
        );
        // Unknown / default profile == defaults.
        assert_eq!(
            code_activation_profile("default").edge_weights[&edge_category::CALLS],
            def.edge_weights[&edge_category::CALLS]
        );
        assert_eq!(
            code_activation_profile("nonsense").edge_weights[&edge_category::CALLS],
            def.edge_weights[&edge_category::CALLS]
        );
    }
}
