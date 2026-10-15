//! Text and row presentation derived from controller state; no worker or window effects.

use cleaner_core::controller::{CleanProgress, Row, Summary};
use cleaner_core::results::{CleanStatus, ScanResult};
use gpui::Hsla;

use super::Controller;
use super::controls::{ICON_CHECK, ICON_ERROR, ICON_WARNING};
use crate::{format, targets::CATEGORIES, theme::Theme};

pub(super) struct Header {
    pub(super) overview: String,
    pub(super) note: String,
    pub(super) estimate: Option<String>,
    pub(super) pending: bool,
    pub(super) unscanned: bool,
    pub(super) accessible_name: String,
}

pub(super) fn header(summary: &Summary, active: Option<&str>) -> Header {
    let unscanned = !summary.scanning && summary.scanned_targets == 0;
    let overview = if summary.scanning {
        active.map_or_else(
            || "Scanning…".to_string(),
            |name| format!("Scanning {name}…"),
        )
    } else if unscanned {
        "Scan to estimate the space you can reclaim".to_string()
    } else if summary.found_targets == 0 && summary.incomplete == 0 {
        "No eligible files found".to_string()
    } else if summary.found_targets == 0 {
        "No complete estimates available".to_string()
    } else {
        format!(
            "{} estimated across {}",
            format::size(summary.complete_bytes),
            format::plural(summary.found_targets as u64, "Target")
        )
    };
    let scan_note = if summary.scanning {
        format!(
            "{} of {} scanned",
            summary.scanned_targets, summary.total_targets
        )
    } else if unscanned {
        "Not scanned yet".to_string()
    } else if summary.incomplete > 0 {
        format!(
            "Excludes {} with incomplete results",
            format::plural(summary.incomplete as u64, "Target")
        )
    } else {
        "Scan complete".to_string()
    };
    let note = format!(
        "{} selected · {scan_note}",
        format::plural(summary.selected_targets as u64, "Target")
    );
    let estimate = summary
        .selected_bytes
        .filter(|_| !unscanned)
        .map(format::size);
    // Labels take their accessible name from their value. An ellipsis ends the sentence.
    let accessible_name = format!(
        "Selected estimate: {}. {overview}{} {note}",
        estimate.as_deref().unwrap_or(if unscanned {
            "not scanned"
        } else {
            "calculating"
        }),
        if overview.ends_with('…') { "" } else { "." }
    );
    Header {
        overview,
        note,
        estimate,
        pending: summary.selected_pending,
        unscanned,
        accessible_name,
    }
}

/// The line under the checklist: startup, errors, Clean progress, or the last result.
pub(super) fn status_line(controller: &Controller, summary: &Summary) -> Option<StatusLine> {
    if controller.is_closing() {
        return Some(StatusLine::info("Stopping…"));
    }
    if let Some(error) = controller.selection_error() {
        return Some(StatusLine::error(error));
    }
    if !controller.is_selection_loaded() {
        return Some(StatusLine::info("Loading Selection…"));
    }
    if controller.selection_locked() {
        return Some(StatusLine::info(format!(
            "Cleaning… {} of {} Targets finished",
            summary.cleaned_targets, summary.clean_targets
        )));
    }
    controller
        .last_clean()
        .map(|results| StatusLine::info(format::clean_summary(results)))
}

/// The shown rows' indices under each Category heading, in checklist order.
/// Absent Targets are hidden, and so is a Category with no shown rows.
pub(super) fn sections(rows: &[Row]) -> Vec<(&'static str, Vec<usize>)> {
    CATEGORIES
        .into_iter()
        .filter_map(|category| {
            let shown: Vec<usize> = (0..rows.len())
                .filter(|&ix| rows[ix].target.category == category && rows[ix].is_visible())
                .collect();
            (!shown.is_empty()).then_some((category, shown))
        })
        .collect()
}

/// The text under the checklist. An error renders like a row error.
pub(super) struct StatusLine {
    pub(super) text: String,
    pub(super) error: bool,
}

impl StatusLine {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
        }
    }
}

pub(super) struct RowStatus {
    pub(super) value: String,
    pub(super) value_color: Hsla,
    pub(super) icon: Option<(&'static str, Hsla)>,
    pub(super) detail: Option<String>,
    pub(super) detail_color: Hsla,
}

/// What a row shows on the right, and the optional explanation under its name.
/// During a Scan, a row not yet reached shows `Waiting`; the `active` row is the
/// one being scanned. Clean results stay shown until fresh Scan results arrive.
pub(super) fn row_status(row: &Row, scanning: bool, active: bool, t: &Theme) -> RowStatus {
    let mut s = RowStatus {
        value: String::new(),
        value_color: t.text_secondary,
        icon: None,
        detail: None,
        detail_color: t.text_secondary,
    };
    let blocked = |detail: String| {
        if row.selected {
            format!("{detail} · Untick or rescan to Clean")
        } else {
            detail
        }
    };
    match (&row.clean, &row.scan) {
        (Some(CleanProgress::Queued), _) => s.value = "Queued".into(),
        (Some(CleanProgress::Cleaning), _) => {
            s.value = "Cleaning…".into();
            s.value_color = t.text;
        }
        (Some(CleanProgress::Done(result)), _) => {
            let deleted = result
                .deleted_bytes
                .map(|bytes| format!("Deleted {}", format::size(bytes)));
            let mut notes: Vec<String> = result
                .skipped
                .iter()
                .map(|&(problem, count)| {
                    format!(
                        "{} skipped ({})",
                        format::plural(count, "file"),
                        format::problem_text(problem)
                    )
                })
                .collect();
            if let Some(problem) = result.coverage_problem {
                notes.push(match result.status {
                    CleanStatus::Failed => sentence(format::problem_text(problem)),
                    _ => format!("Incomplete: {}", format::problem_text(problem)),
                });
            }
            if deleted.is_none() && result.status != CleanStatus::Failed {
                notes.push("Size unavailable".into());
            }
            match result.status {
                CleanStatus::Complete => {
                    s.icon = Some((ICON_CHECK, t.success));
                    s.value = deleted.unwrap_or_else(|| "Emptied".into());
                }
                CleanStatus::Partial => {
                    s.icon = Some((ICON_WARNING, t.caution));
                    s.value = deleted.unwrap_or_else(|| "Partly emptied".into());
                }
                CleanStatus::Failed => {
                    s.icon = Some((ICON_ERROR, t.critical));
                    s.value = "Clean failed".into();
                }
                CleanStatus::Stopped => {
                    s.value = "Stopped".into();
                    if let Some(deleted) = deleted {
                        notes.push(format!("{deleted} before stopping"));
                    }
                }
            }
            s.detail = (!notes.is_empty()).then(|| notes.join(" · "));
        }
        (None, None) if active => {
            s.value = "Scanning…".into();
            s.value_color = t.text;
        }
        (None, None) => {
            s.value = if scanning { "Waiting" } else { "Not scanned" }.into();
            s.value_color = t.text_disabled;
        }
        (None, Some(ScanResult::Complete { bytes })) => s.value = format::size(*bytes),
        (None, Some(ScanResult::NotPresent)) => s.value = "Not present".into(),
        (None, Some(ScanResult::Partial { bytes, problem })) => {
            s.icon = Some((ICON_WARNING, t.caution));
            s.value = format!("{} (incomplete)", format::size(*bytes));
            s.detail = Some(blocked(sentence(format::problem_text(*problem))));
            s.detail_color = t.caution;
        }
        (None, Some(ScanResult::Failed { problem })) => {
            s.icon = Some((ICON_ERROR, t.critical));
            s.value = "Scan failed".into();
            s.detail = Some(blocked(sentence(format::problem_text(*problem))));
            s.detail_color = t.critical;
        }
        (None, Some(ScanResult::Stopped)) => {
            s.value = "Not scanned".into();
            s.detail = Some(blocked("Scan stopped".into()));
        }
    }
    s
}

fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::targets;

    #[test]
    fn absent_targets_and_empty_categories_are_hidden() {
        let rows: Vec<Row> = targets::catalog()
            .map(|target| Row {
                target,
                selected: true,
                scan: Some(match target.id {
                    "chrome-cache" | "directx-shader-cache" => ScanResult::NotPresent,
                    _ => ScanResult::Complete { bytes: 1 },
                }),
                clean: None,
            })
            .collect();
        let shown: Vec<(&str, Vec<&str>)> = sections(&rows)
            .into_iter()
            .map(|(category, indices)| {
                (
                    category,
                    indices.iter().map(|&ix| rows[ix].target.id).collect(),
                )
            })
            .collect();
        let headings: Vec<&str> = shown.iter().map(|(name, _)| *name).collect();
        assert_eq!(headings, ["Windows", "Developer"]);
        assert_eq!(
            shown[0].1,
            [
                "user-temp",
                "windows-temp",
                "recycle-bin",
                "thumbnail-cache",
                "crash-dumps"
            ]
        );
    }
}
