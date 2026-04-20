//! Context-window compaction primitives.
//!
//! Reusable types and helpers for splitting a message list into a
//! to-summarize head and a kept tail. Agent-specific summary formats
//! and file-operation extraction remain in `agent_loop::compaction`.

pub mod helpers;
pub mod micro;
pub mod summary;
pub mod types;

pub use self::helpers::{
    collect_entries_between_branches, find_compaction_cut_index, prepare_compaction,
};
pub use self::micro::{compact_micro, compact_micro_with_config, MicroCompactionConfig};
pub use self::summary::assemble_summary_compaction;
pub use self::types::{
    BranchEntryRange, CompactionRequest, CompactionResult, CompactionSpan, CompactionStrategy,
    CompactionSuffix, FileOperationSnapshot, MicroCompactionRequest, MicroCompactionResult,
    PreparedCompaction, SummaryArtifact, SummaryCompactionRequest, SummaryCompactionResult,
};
