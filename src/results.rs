//! Per-Target Scan and Clean results, as reported by a worker.

/// Why content could not be inspected or cleaned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    AccessDenied,
    SharingViolation,
    Redirected,
}

impl Problem {
    pub fn text(self) -> &'static str {
        match self {
            Problem::AccessDenied => "access denied",
            Problem::SharingViolation => "sharing violation",
            Problem::Redirected => "redirected folder skipped",
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanResult {
    pub status: CleanStatus,
    /// Logical bytes accepted for deletion, or `None` when the procedure cannot report them.
    pub deleted_bytes: Option<u64>,
    /// Known counts of files skipped, by reason.
    pub skipped: Vec<(Problem, u64)>,
    /// A folder or subtree that could not be covered.
    pub coverage_problem: Option<Problem>,
}
