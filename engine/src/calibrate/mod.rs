//! The per-repo calibration (0.5.3 items 4, 8, 10 and 11): a leak-free online
//! replay of the repo's own history over fixed candidate rankings, persisted
//! as `.glia/calibration-snapshot/` by `glia history calibrate` and read by
//! the ranked answers through `select`. A snapshot step, never part of a
//! build. Owners: EC.1-EC.6, EC.10-EC.13; each adds its own `pub mod` and
//! `pub use` lines here.

pub mod record;
pub use record::{
    CalibratedSet, Calibration, CalibrationHandle, CalibrationMeta, CalibrationRead, CommitScore,
    OnlineReplay, ReplayParamsRecord, Selection, Shape, explicit, load_for_roots, log_selection,
    read_calibration, select, set_digest, write_calibration,
};

pub mod replay;
pub use replay::{
    CoState, FIND_TOP, Replay, ReplayCommit, ReplayParams, ReplayQuery, build_replay, find_top,
    find_top_scores, seed_order_hash,
};

pub mod candidate;
pub use candidate::{Candidate, Memo, QueryCtx, RankKey, ReplayState, StateHook};

pub mod select;
pub use select::{SetSpec, better, eval_query, run_sets};

pub mod bank;
pub use bank::{
    StructIndex, bfs_files, ppr_files, structure_keys, uniform_seed_nodes, weighted_seed_nodes,
};

pub mod sets;
pub use sets::{
    COCHANGE_CANDIDATES, COCHANGE_FALLBACK, COCHANGE_SET, Named, SEEDED_SHAPES,
    STRUCTURE_CANDIDATES, STRUCTURE_FALLBACK, STRUCTURE_SET, specs,
};

pub mod step;
pub use step::{
    BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, CalibrateOptions, CalibrationSummary, DEFAULT_GROUPS,
    SetSummary, bootstrap_ci, calibrate, calibrate_snapshot,
};
