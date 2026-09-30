//! Contract breaks against a git rev (CC.8a, + CC.8c): every schema copy and
//! contract op paired old -> new (same identity FACT, through a move DERIVED)
//! and judged by its format's evolution rules, plus the clients left without
//! a provider; CC.8c builds the rev pair as two multi-repo merges so clients
//! in other repos (`--with`) are reported too. Public slot, reached by module
//! path (`glia_engine::contract_breaks::<item>`). Filled by CC.8a.
