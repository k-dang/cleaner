//! Per-Target Scan and Clean results, as reported by a worker.

use crate::targets::TargetId;

/// Why content could not be inspected or cleaned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    AccessDenied,
    SharingViolation,
    Redirected,
    Metadata,
    NonLocal,
    /// The cleanup location no longer matches the location inspected by Scan.
    LocationChanged,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanResult {
    /// Every applicable folder was inspected. `bytes` is the eligible logical size.
    Complete {
        bytes: u64,
    },
    /// Every configured folder is confirmed absent.
    NotPresent,
    /// Some content was inspected; `bytes` covers only that content.
    Partial {
        bytes: u64,
        problem: Problem,
    },
    Failed {
        problem: Problem,
    },
    /// The operation stopped before this Target finished.
    Stopped,
}

/// How far a Target's Clean got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanStatus {
    Complete,
    Partial,
    Failed,
    Stopped,
}

/// Adds `count` to `problem`'s entry, keeping first-seen order.
pub fn add_count(counts: &mut Vec<(Problem, u64)>, problem: Problem, count: u64) {
    match counts.iter_mut().find(|(p, _)| *p == problem) {
        Some((_, total)) => *total += count,
        None => counts.push((problem, count)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanResult {
    pub status: CleanStatus,
    /// Known logical bytes accepted for deletion. `None` when the cleanup
    /// procedure reports no per-file results.
    pub deleted_bytes: Option<u64>,
    /// Known counts of files skipped, by reason.
    pub skipped: Vec<(Problem, u64)>,
    /// A folder or subtree that could not be covered.
    pub coverage_problem: Option<Problem>,
}

/// A worker's report about the operation it is running, in the order it happens.
#[derive(Debug, PartialEq, Eq)]
pub enum Event<Snapshot> {
    Scanning(TargetId),
    /// The native snapshot is retained for this Target and returned unchanged to Clean.
    Scanned(TargetId, ScanResult, Snapshot),
    Cleaning(TargetId),
    Cleaned(TargetId, CleanResult),
    /// The worker has stopped touching the filesystem for this operation.
    Finished,
}
