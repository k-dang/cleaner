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
    /// The drive letter mounts a different volume than the Scan found.
    DriveChanged,
    Other,
}

impl Problem {
    pub fn text(self) -> &'static str {
        match self {
            Problem::AccessDenied => "access denied",
            Problem::SharingViolation => "sharing violation",
            Problem::Redirected => "redirected folder skipped",
            Problem::Metadata => "metadata unavailable",
            Problem::NonLocal => "network location skipped",
            Problem::DriveChanged => "drive changed since Scan",
            Problem::Other => "I/O error",
        }
    }
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
    /// procedure reports no per-file results, as the Recycle Bin's shell call does.
    pub deleted_bytes: Option<u64>,
    /// Known counts of files skipped, by reason.
    pub skipped: Vec<(Problem, u64)>,
    /// A folder or subtree that could not be covered.
    pub coverage_problem: Option<Problem>,
}

/// A drive whose Recycle Bin a Scan covered: its letter and the volume mounted
/// there, as a volume GUID path such as `\\?\Volume{...}\`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    pub letter: char,
    pub volume: String,
}

/// A worker's report about the operation it is running, in the order it happens.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Scanned(TargetId, ScanResult),
    /// The Recycle Bin's Scan, with the drives it covered. A Clean empties only these.
    RecycleBinScanned(TargetId, ScanResult, Vec<Drive>),
    Cleaning(TargetId),
    Cleaned(TargetId, CleanResult),
    /// The worker has stopped touching the filesystem for this operation.
    Finished,
}
