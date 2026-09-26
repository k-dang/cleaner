//! The fixed table of built-in Targets, in checklist order.

/// A checklist heading that groups Targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Windows,
    Browsers,
    Apps,
    Developer,
}

impl Category {
    pub const ALL: [Category; 4] = [
        Category::Windows,
        Category::Browsers,
        Category::Apps,
        Category::Developer,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Category::Windows => "Windows",
            Category::Browsers => "Browsers",
            Category::Apps => "Apps",
            Category::Developer => "Developer",
        }
    }
}

pub type TargetId = &'static str;

/// A built-in cleanup choice, shown as one checklist row.
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    pub id: TargetId,
    pub name: &'static str,
    pub category: Category,
    pub default_selected: bool,
}

const fn target(
    id: TargetId,
    name: &'static str,
    category: Category,
    default_selected: bool,
) -> Target {
    Target {
        id,
        name,
        category,
        default_selected,
    }
}

use Category::*;

pub static TARGETS: [Target; 20] = [
    target("user-temp", "User temp", Windows, true),
    target("windows-temp", "Windows temp", Windows, true),
    target("recycle-bin", "Recycle Bin", Windows, true),
    target("thumbnail-cache", "Thumbnail and icon cache", Windows, true),
    target(
        "crash-dumps",
        "Crash dumps and error reports",
        Windows,
        true,
    ),
    target(
        "directx-shader-cache",
        "DirectX shader cache",
        Windows,
        false,
    ),
    target("chrome-cache", "Chrome cache", Browsers, true),
    target("edge-cache", "Edge cache", Browsers, true),
    target("brave-cache", "Brave cache", Browsers, true),
    target("firefox-cache", "Firefox cache", Browsers, true),
    target("discord-cache", "Discord cache", Apps, true),
    target("vscode-cache", "VS Code cache", Apps, true),
    target("nvidia-shader-cache", "NVIDIA shader cache", Apps, false),
    target("npm-cache", "npm cache", Developer, false),
    target("pnpm-store", "pnpm store", Developer, false),
    target("bun-cache", "Bun cache", Developer, false),
    target("yarn-cache", "Yarn cache", Developer, false),
    target("pip-cache", "pip cache", Developer, false),
    target("cargo-registry", "Cargo registry", Developer, false),
    target("go-build-cache", "Go build cache", Developer, false),
];
