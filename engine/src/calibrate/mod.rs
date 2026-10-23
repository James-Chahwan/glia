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
    COCHANGE_CANDIDATES, COCHANGE_FALLBACK, COCHANGE_SET, FIND_CANDIDATES, FIND_FALLBACK,
    FIND_SET, FIND_SHAPES, GROUPS, Named, REVIEW_CANDIDATES,
    REVIEW_FALLBACK, REVIEW_SET, SEEDED_SHAPES, STRUCTURE_CANDIDATES, STRUCTURE_FALLBACK,
    STRUCTURE_SET, log_hooks, register_hooks, specs,
};

pub mod fusions;
pub use fusions::{
    AnswerInputs, FIND_NAMES, FindHead, FindNeeds, FindSeeded, FindSignals, HT_GRID, LX_IDR_H9S50, MEMO_T1S, Needs, SEEDED_NAMES, Seeded, Signals,
    TextState, answer_signals, find_keys, find_needs, find_order, find_signals_at_head, needs,
    replay_find_signals, replay_signals, seeded_keys, seeded_order,
};

pub mod step;
pub use step::{
    BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, CalibrateOptions, CalibrationSummary, DEFAULT_GROUPS,
    SetSummary, bootstrap_ci, calibrate, calibrate_snapshot,
};

pub mod work;
pub use work::{
    GAP_S, MEMO_M, MEMO_W5, OVERLAP_LAST, W_DECAY, W_K, WorkState, m_scores, w5_scores,
    work_at_head,
};
