//! The fixed table of built-in Targets, in checklist order.

pub type TargetId = &'static str;

/// A built-in cleanup choice, shown as one checklist row.
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    pub id: TargetId,
    pub name: &'static str,
    pub default_selected: bool,
}

pub static TARGETS: [Target; 2] = [
    Target {
        id: "user-temp",
        name: "User temp",
        default_selected: true,
    },
    Target {
        id: "windows-temp",
        name: "Windows temp",
        default_selected: true,
    },
];
