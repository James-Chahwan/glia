//! `glia cache` (CE.2c, +CE.2d, CE.2e): `push` / `pull` / `gc` of the shared
//! build cache, the CLI-only transport over the engine's keys / export /
//! import (CE.2c: object.rs, store.rs, gc.rs; CE.2d: layout.rs, `--layout`;
//! CE.2e: http.rs). The owner adds `Args`, `run`, the `StoreCmd` variant
//! (its doc comment is the clap about) and its match arm.
