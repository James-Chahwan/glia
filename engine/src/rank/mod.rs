//! The query-time ranking of files for the structural answers (0.5.3 items
//! 1, 2, 5 and 10): the PPR index cache (`RankCache`), the candidate file
//! space (`FileIndex`, rank_eval's `index_graph` rule), co-change from the
//! history snapshot (`CochangeIndex`), the measured file ranking (`Ranking` /
//! `RankBase`, `rank_files`) and the row order the answers apply
//! (`order_rows`). Nothing here runs inside a build; ties go by (repo label,
//! path), never a NodeId. Owners: EA.2 (ranking, cache), EA.3 (files,
//! cochange), EA.4 (order); each adds its own `mod` and `pub use` lines here.

mod cache;
mod cochange;
mod files;
mod order;
mod ranking;

pub use cache::{RankCache, RankCacheStats, SourceKey};
pub use cochange::{CochangeHistory, CochangeIndex, CochangeStats, c0};
pub use files::{FileIndex, NOT_CODE_EXTS};
pub use order::{FileRanking, RankedFile, base_keys, order_rows, rank_files, rank_files_with_c0};
pub use ranking::{RankBase, Ranking, STRUCTURE_CANDIDATES, WIDE_DAMPING};
