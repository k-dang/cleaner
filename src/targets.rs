//! The fixed table of built-in Targets, in checklist order. The core resolves
//! each Target's folders against `Roots`; nothing outside this table can add a path.

use std::time::Duration;

pub type TargetId = &'static str;

/// A checklist heading. Targets are listed in `Category::ALL` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Windows,
    Browsers,
    Developer,
}

impl Category {
    pub const ALL: [Category; 3] = [Category::Windows, Category::Browsers, Category::Developer];

    pub fn name(self) -> &'static str {
        match self {
            Category::Windows => "Windows",
            Category::Browsers => "Browsers",
            Category::Developer => "Developer",
        }
    }
}

/// A known folder that Target paths are relative to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    LocalAppData,
    WinDir,
    ProgramData,
}

/// True for the Chromium profile directories `Default` and `Profile <number>`.
pub fn is_chromium_profile(name: &str) -> bool {
    const PREFIX: &str = "Profile ";
    name.eq_ignore_ascii_case("Default")
        || name
            .get(..PREFIX.len())
            .zip(name.get(PREFIX.len()..))
            .is_some_and(|(prefix, number)| {
                prefix.eq_ignore_ascii_case(PREFIX)
                    && !number.is_empty()
                    && number.bytes().all(|b| b.is_ascii_digit())
            })
}

/// Where a Target's eligible content lives. Paths are relative to their `Base`.
/// Every listed folder is preserved; only its eligible contents are removed.
#[derive(Debug, PartialEq, Eq)]
pub enum Folders {
    /// Everything inside each folder. A missing folder is normal while another exists.
    Trees(&'static [(Base, &'static str)]),
    /// Only immediate files named `prefix*suffix` (ASCII case-insensitive).
    Files {
        base: Base,
        path: &'static str,
        prefix: &'static str,
        suffix: &'static str,
    },
    /// Everything inside the fixed `caches` folders of each Chromium profile
    /// directly under `path`.
    Profiles {
        base: Base,
        path: &'static str,
        caches: &'static [&'static str],
    },
}

/// A built-in cleanup choice, shown as one checklist row.
#[derive(Debug, PartialEq, Eq)]
pub struct Target {
    pub id: TargetId,
    pub name: &'static str,
    pub category: Category,
    pub default_selected: bool,
    /// Files modified more recently than this before the operation starts are kept.
    pub min_age: Option<Duration>,
    pub folders: Folders,
}

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Returns the built-in Target with this ID.
pub fn find(id: &str) -> Option<&'static Target> {
    TARGETS.iter().find(|target| target.id == id)
}

// Admission evidence for each Target is recorded in
// `.scratch/cleaner-v1/issues/03-add-folder-based-targets.md`.
pub static TARGETS: [Target; 9] = [
    Target {
        id: "user-temp",
        name: "User temp",
        category: Category::Windows,
        default_selected: true,
        min_age: Some(DAY),
        folders: Folders::Trees(&[(Base::LocalAppData, "Temp")]),
    },
    Target {
        id: "windows-temp",
        name: "Windows temp",
        category: Category::Windows,
        default_selected: true,
        min_age: Some(DAY),
        folders: Folders::Trees(&[(Base::WinDir, "Temp")]),
    },
    Target {
        id: "thumbnail-cache",
        name: "Thumbnail cache",
        category: Category::Windows,
        default_selected: true,
        min_age: None,
        folders: Folders::Files {
            base: Base::LocalAppData,
            path: r"Microsoft\Windows\Explorer",
            prefix: "thumbcache_",
            suffix: ".db",
        },
    },
    Target {
        id: "crash-dumps",
        name: "Crash dumps and error reports",
        category: Category::Windows,
        default_selected: true,
        min_age: None,
        folders: Folders::Trees(&[
            (Base::LocalAppData, "CrashDumps"),
            (Base::LocalAppData, r"Microsoft\Windows\WER"),
            (Base::ProgramData, r"Microsoft\Windows\WER\ReportArchive"),
            (Base::ProgramData, r"Microsoft\Windows\WER\ReportQueue"),
        ]),
    },
    Target {
        id: "directx-shader-cache",
        name: "DirectX shader cache",
        category: Category::Windows,
        default_selected: false,
        min_age: None,
        folders: Folders::Trees(&[(Base::LocalAppData, "D3DSCache")]),
    },
    Target {
        id: "chrome-cache",
        name: "Chrome cache",
        category: Category::Browsers,
        default_selected: true,
        min_age: None,
        folders: Folders::Profiles {
            base: Base::LocalAppData,
            path: r"Google\Chrome\User Data",
            caches: &["Cache", "Code Cache", "GPUCache"],
        },
    },
    Target {
        id: "pnpm-store",
        name: "pnpm store",
        category: Category::Developer,
        default_selected: false,
        min_age: None,
        folders: Folders::Trees(&[(Base::LocalAppData, r"pnpm\store")]),
    },
    Target {
        id: "pip-cache",
        name: "pip cache",
        category: Category::Developer,
        default_selected: false,
        min_age: None,
        folders: Folders::Trees(&[(Base::LocalAppData, r"pip\cache")]),
    },
    Target {
        id: "go-build-cache",
        name: "Go build cache",
        category: Category::Developer,
        default_selected: false,
        min_age: None,
        folders: Folders::Trees(&[(Base::LocalAppData, "go-build")]),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_profiles_are_default_and_numbered_profiles_only() {
        for name in ["Default", "Profile 1", "Profile 12"] {
            assert!(is_chromium_profile(name), "{name}");
        }
        for name in [
            "Guest Profile",
            "System Profile",
            "Profile ",
            "Profile 1a",
            "Profile",
            "Defaults",
        ] {
            assert!(!is_chromium_profile(name), "{name}");
        }
    }

    #[test]
    fn target_ids_are_unique() {
        for (ix, target) in TARGETS.iter().enumerate() {
            assert!(TARGETS[ix + 1..].iter().all(|other| other.id != target.id));
        }
    }
}
