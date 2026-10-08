//! Checklist metadata supplied by the native app. Cleanup paths and procedures
//! belong to that app, rather than the shared workflow.

use std::time::Duration;

pub type TargetId = &'static str;

/// A built-in cleanup choice, shown as one checklist row.
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    pub id: TargetId,
    pub name: &'static str,
    pub category: &'static str,
    pub default_selected: bool,
    /// Files modified more recently than this before an operation starts are kept.
    pub min_age: Option<Duration>,
}
