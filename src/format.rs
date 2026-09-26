//! User-facing text for sizes and Clean results.

use crate::results::{CleanResult, CleanStatus, Problem};
use crate::targets::Target;

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
pub fn clean_summary(results: &[(&Target, CleanResult)]) -> String {
    let mut deleted = 0;
    let mut skipped: Vec<(Problem, u64)> = Vec::new();
    let mut unavailable = Vec::new();
    let mut incomplete: Vec<(Problem, u64)> = Vec::new();
    let mut stopped = 0;
    for (target, result) in results {
        match result.deleted_bytes {
            Some(bytes) => deleted += bytes,
            None if result.status == CleanStatus::Complete => {
                unavailable.push(format!("{} emptied (size unavailable)", target.name));
            }
            None => {}
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

    let mut parts = vec![format!("Deleted {} of files", size(deleted))];
    for (problem, count) in skipped {
        parts.push(format!(
            "{} skipped ({})",
            plural(count, "file"),
            problem.text()
        ));
    }
    parts.extend(unavailable);
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

/// Adds `count` to `problem`'s entry, keeping first-seen order.
fn add_count(counts: &mut Vec<(Problem, u64)>, problem: Problem, count: u64) {
    match counts.iter_mut().find(|(p, _)| *p == problem) {
        Some((_, total)) => *total += count,
        None => counts.push((problem, count)),
    }
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
    use crate::targets::TARGETS;

    fn target(id: &str) -> &'static Target {
        TARGETS.iter().find(|t| t.id == id).unwrap()
    }

    fn result(status: CleanStatus, deleted: Option<u64>) -> CleanResult {
        CleanResult {
            status,
            deleted_bytes: deleted,
            skipped: vec![],
            coverage_problem: None,
        }
    }

    #[test]
    fn summary_reports_skipped_files_by_reason() {
        let mut temp = result(CleanStatus::Partial, Some(1229 * MB));
        temp.skipped = vec![(Problem::SharingViolation, 3)];
        let mut edge = result(CleanStatus::Partial, Some(0));
        edge.skipped = vec![(Problem::SharingViolation, 1)];
        assert_eq!(
            clean_summary(&[(target("user-temp"), temp), (target("edge-cache"), edge)]),
            "Deleted 1.2 GB of files · 4 files skipped (sharing violation)"
        );
    }

    #[test]
    fn summary_marks_unavailable_sizes_instead_of_counting_zero() {
        assert_eq!(
            clean_summary(&[
                (
                    target("user-temp"),
                    result(CleanStatus::Complete, Some(1229 * MB))
                ),
                (target("recycle-bin"), result(CleanStatus::Complete, None)),
            ]),
            "Deleted 1.2 GB of files · Recycle Bin emptied (size unavailable)"
        );
    }

    #[test]
    fn summary_counts_incomplete_targets_by_problem() {
        let mut denied = result(CleanStatus::Partial, Some(640 * MB));
        denied.coverage_problem = Some(Problem::AccessDenied);
        let mut failed = result(CleanStatus::Failed, Some(0));
        failed.coverage_problem = Some(Problem::AccessDenied);
        let mut one_skip = result(CleanStatus::Partial, Some(0));
        one_skip.skipped = vec![(Problem::AccessDenied, 1)];
        assert_eq!(
            clean_summary(&[
                (target("windows-temp"), denied),
                (target("crash-dumps"), failed),
                (target("chrome-cache"), one_skip),
                (target("edge-cache"), result(CleanStatus::Stopped, Some(0))),
            ]),
            "Deleted 640 MB of files · 1 file skipped (access denied) · 2 Targets incomplete (access denied) · 1 Target stopped"
        );
    }

    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    #[test]
    fn sizes_use_one_decimal_below_ten_and_whole_numbers_above() {
        assert_eq!(size(0), "0 bytes");
        assert_eq!(size(1), "1 byte");
        assert_eq!(size(1023), "1023 bytes");
        assert_eq!(size(1536), "1.5 KB");
        assert_eq!(size(604 * MB), "604 MB");
        assert_eq!(size(1434 * MB), "1.4 GB");
        assert_eq!(size(12 * GB), "12 GB");
    }

    #[test]
    fn rounding_never_shows_a_unit_overflow() {
        assert_eq!(size(10 * KB - 1), "10 KB");
        assert_eq!(size(MB - 1), "1.0 MB");
    }
}
