//! Simulated Scan and Clean results for the UI while real Targets are validated.
//! Nothing here touches the filesystem.

use std::collections::HashSet;
use std::time::Duration;

use crate::results::{CleanResult, CleanStatus, Problem, ScanResult};
use crate::targets::TargetId;

const MB: u64 = 1024 * 1024;
const GB: u64 = 1024 * MB;

/// Remembers which Targets were "cleaned" so the next Scan shows smaller sizes.
#[derive(Default)]
pub struct Fixture {
    cleaned: HashSet<TargetId>,
}

impl Fixture {
    /// How long a Target takes to Scan, and its result.
    pub fn scan(&self, id: TargetId) -> (Duration, ScanResult) {
        let complete = |bytes| ScanResult::Complete { bytes };
        let (ms, result) = match id {
            "user-temp" => (900, complete(2150 * MB)),
            "windows-temp" => (600, complete(812 * MB)),
            "recycle-bin" => (300, complete(1434 * MB)),
            "thumbnail-cache" => (400, complete(604 * MB)),
            "crash-dumps" => (500, complete(96 * MB)),
            "directx-shader-cache" => (
                300,
                ScanResult::Failed {
                    problem: Problem::AccessDenied,
                },
            ),
            "chrome-cache" => (1200, complete(1843 * MB)),
            "edge-cache" => (700, complete(736 * MB)),
            "firefox-cache" => (400, complete(212 * MB)),
            "discord-cache" => (300, complete(318 * MB)),
            "vscode-cache" => (300, complete(455 * MB)),
            "nvidia-shader-cache" => (800, complete(12 * GB)),
            "npm-cache" => (1500, complete(3277 * MB)),
            // Slow and unticked, so Clean can start while it is still scanning.
            "pnpm-store" => (15_000, complete(5734 * MB)),
            "bun-cache" => (900, complete(1126 * MB)),
            "pip-cache" => (300, complete(245 * MB)),
            "cargo-registry" => (
                700,
                ScanResult::Partial {
                    bytes: 2458 * MB,
                    problem: Problem::Redirected,
                },
            ),
            _ => (100, ScanResult::NotPresent),
        };
        let result = match (result, self.cleaned.contains(id)) {
            (ScanResult::Complete { .. }, true) => complete(leftover(id)),
            (result, _) => result,
        };
        (Duration::from_millis(ms), result)
    }

    /// How long a Target takes to Clean, and its result.
    pub fn clean(&mut self, id: TargetId, estimate: u64) -> (Duration, CleanResult) {
        let mut result = CleanResult {
            status: CleanStatus::Complete,
            deleted_bytes: Some(estimate),
            skipped: vec![],
            coverage_problem: None,
        };
        match id {
            "user-temp" => {
                result.status = CleanStatus::Partial;
                result.deleted_bytes = Some(estimate - leftover(id));
                result.skipped = vec![(Problem::SharingViolation, 4)];
            }
            "windows-temp" => {
                result.status = CleanStatus::Partial;
                result.deleted_bytes = Some(estimate - leftover(id));
                result.coverage_problem = Some(Problem::AccessDenied);
            }
            "crash-dumps" => {
                result.status = CleanStatus::Failed;
                result.deleted_bytes = Some(0);
                result.coverage_problem = Some(Problem::AccessDenied);
            }
            // The shell empties the bin without reporting sizes.
            "recycle-bin" => result.deleted_bytes = None,
            _ => {}
        }
        if result.status != CleanStatus::Failed {
            self.cleaned.insert(id);
        }
        (Duration::from_millis(1200), result)
    }
}

/// What a Target still holds after a fixture Clean.
fn leftover(id: TargetId) -> u64 {
    match id {
        "user-temp" => 38 * MB,
        "windows-temp" => 172 * MB,
        _ => 0,
    }
}
