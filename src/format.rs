//! User-facing text for sizes and Clean results.

use crate::results::{CleanResult, CleanStatus, Problem, add_count};

/// Formats a byte count the way Windows Explorer does (1024-based units).
pub fn size(bytes: u64) -> String {
    match bytes {
        1 => return "1 byte".into(),
        0..1024 => return format!("{bytes} bytes"),
        _ => {}
    }
    let mut value = bytes as f64;
    for unit in ["KB", "MB", "GB", "TB"] {
        value /= 1024.0;
        if value < 9.95 {
            return format!("{value:.1} {unit}");
        }
        if value.round() < 1024.0 || unit == "TB" {
            return format!("{value:.0} {unit}");
        }
    }
    unreachable!("the TB arm always returns")
}

/// The one-line summary of a finished Clean, e.g.
/// `Deleted 1.2 GB of files · 4 files skipped (sharing violation)`.
pub fn clean_summary(results: &[CleanResult]) -> String {
    let mut deleted = None;
    let mut emptied = false;
    let mut skipped: Vec<(Problem, u64)> = Vec::new();
    let mut incomplete: Vec<(Problem, u64)> = Vec::new();
    let mut stopped = 0;
    for result in results {
        match result.deleted_bytes {
            Some(bytes) => *deleted.get_or_insert(0) += bytes,
            None => emptied |= result.status == CleanStatus::Complete,
        }
        for &(problem, count) in &result.skipped {
            add_count(&mut skipped, problem, count);
        }
        if result.status == CleanStatus::Stopped {
            stopped += 1;
        } else if let Some(problem) = result.coverage_problem {
            add_count(&mut incomplete, problem, 1);
        }
    }

    let mut parts = Vec::new();
    if let Some(deleted) = deleted {
        parts.push(format!("Deleted {} of files", size(deleted)));
    }
    if emptied {
        parts.push("Recycle Bin emptied (size unavailable)".into());
    }
    for (problem, count) in skipped {
        parts.push(format!(
            "{} skipped ({})",
            plural(count, "file"),
            problem.text()
        ));
    }
    for (problem, count) in incomplete {
        parts.push(format!(
            "{} incomplete ({})",
            plural(count, "Target"),
            problem.text()
        ));
    }
    if stopped > 0 {
        parts.push(format!("{} stopped", plural(stopped, "Target")));
    }
    parts.join(" · ")
}

/// `1 file`, `2 files`.
pub fn plural(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reports_known_deletions_and_rejected_files() {
        let result = CleanResult {
            status: CleanStatus::Partial,
            deleted_bytes: Some(1229 * 1024 * 1024),
            skipped: vec![(Problem::SharingViolation, 4)],
            coverage_problem: None,
        };
        assert_eq!(
            clean_summary(&[result]),
            "Deleted 1.2 GB of files · 4 files skipped (sharing violation)"
        );
    }

    #[test]
    fn recycle_bin_outcome_is_reported_without_a_size() {
        let files = CleanResult {
            status: CleanStatus::Complete,
            deleted_bytes: Some(1229 * 1024 * 1024),
            skipped: vec![],
            coverage_problem: None,
        };
        let emptied = CleanResult {
            deleted_bytes: None,
            ..files.clone()
        };
        assert_eq!(
            clean_summary(&[files.clone(), emptied.clone()]),
            "Deleted 1.2 GB of files · Recycle Bin emptied (size unavailable)"
        );
        assert_eq!(
            clean_summary(std::slice::from_ref(&emptied)),
            "Recycle Bin emptied (size unavailable)"
        );
        let failed = CleanResult {
            status: CleanStatus::Partial,
            coverage_problem: Some(Problem::AccessDenied),
            ..emptied
        };
        assert_eq!(
            clean_summary(&[files, failed]),
            "Deleted 1.2 GB of files · 1 Target incomplete (access denied)"
        );
    }

    #[test]
    fn sizes_are_estimates_in_binary_units() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(1536), "1.5 KB");
        assert_eq!(size(12 * 1024 * 1024 * 1024), "12 GB");
    }
}
