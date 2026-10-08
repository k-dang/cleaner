//! Windows Target catalog and verified cleanup recipes, in checklist order.
//! The portable workflow sees only metadata; native workers own paths and procedures.

use std::time::Duration;

use cleaner_core::targets::Target as Metadata;

/// Native checklist headings, in display order.
pub const CATEGORIES: [&str; 3] = ["Windows", "Browsers", "Developer"];

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

/// What a Target cleans.
#[derive(Debug, PartialEq, Eq)]
pub enum Content {
    Folders {
        folders: Folders,
    },
    /// The current account's Recycle Bin on each mounted local fixed drive,
    /// queried and emptied only through the Windows shell.
    RecycleBin,
}

/// Checklist metadata paired with its Windows cleanup procedure.
#[derive(Debug, PartialEq, Eq)]
pub struct Recipe {
    pub target: Metadata,
    pub content: Content,
}

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// Returns the built-in Target with this ID.
pub fn find(id: &str) -> Option<&'static Recipe> {
    TARGETS.iter().find(|recipe| recipe.target.id == id)
}

/// Metadata accepted by the shared workflow and Selection format.
pub fn catalog() -> impl Iterator<Item = &'static Metadata> {
    TARGETS.iter().map(|recipe| &recipe.target)
}

// Why each Target is admitted, and which were excluded, is in README.md.
pub static TARGETS: [Recipe; 10] = [
    Recipe {
        target: Metadata {
            id: "user-temp",
            name: "User temp",
            category: "Windows",
            default_selected: true,
            min_age: Some(DAY),
        },
        content: Content::Folders {
            folders: Folders::Trees(&[(Base::LocalAppData, "Temp")]),
        },
    },
    Recipe {
        target: Metadata {
            id: "windows-temp",
            name: "Windows temp",
            category: "Windows",
            default_selected: true,
            min_age: Some(DAY),
        },
        content: Content::Folders {
            folders: Folders::Trees(&[(Base::WinDir, "Temp")]),
        },
    },
    Recipe {
        target: Metadata {
            id: "recycle-bin",
            name: "Recycle Bin",
            category: "Windows",
            default_selected: true,
            min_age: None,
        },
        content: Content::RecycleBin,
    },
    Recipe {
        target: Metadata {
            id: "thumbnail-cache",
            name: "Thumbnail cache",
            category: "Windows",
            default_selected: true,
            min_age: None,
        },
        content: Content::Folders {
            folders: Folders::Files {
                base: Base::LocalAppData,
                path: r"Microsoft\Windows\Explorer",
                prefix: "thumbcache_",
                suffix: ".db",
            },
        },
    },
    Recipe {
        target: Metadata {
            id: "crash-dumps",
            name: "Crash dumps and error reports",
            category: "Windows",
            default_selected: true,
            min_age: None,
        },
        content: Content::Folders {
            folders: Folders::Trees(&[
                (Base::LocalAppData, "CrashDumps"),
                (Base::LocalAppData, r"Microsoft\Windows\WER"),
                (Base::ProgramData, r"Microsoft\Windows\WER\ReportArchive"),
                (Base::ProgramData, r"Microsoft\Windows\WER\ReportQueue"),
            ]),
        },
    },
    Recipe {
        target: Metadata {
            id: "directx-shader-cache",
            name: "DirectX shader cache",
            category: "Windows",
            default_selected: false,
            min_age: None,
        },
        content: Content::Folders {
            folders: Folders::Trees(&[(Base::LocalAppData, "D3DSCache")]),
        },
    },
    Recipe {
        target: Metadata {
            id: "chrome-cache",
            name: "Chrome cache",
            category: "Browsers",
            default_selected: true,
            min_age: None,
        },
        content: Content::Folders {
            folders: Folders::Profiles {
                base: Base::LocalAppData,
                path: r"Google\Chrome\User Data",
                caches: &["Cache", "Code Cache", "GPUCache"],
            },
        },
    },
    Recipe {
        target: Metadata {
            id: "pnpm-store",
            name: "pnpm store",
            category: "Developer",
            default_selected: false,
            min_age: None,
        },
        content: Content::Folders {
            folders: Folders::Trees(&[(Base::LocalAppData, r"pnpm\store")]),
        },
    },
    Recipe {
        target: Metadata {
            id: "pip-cache",
            name: "pip cache",
            category: "Developer",
            default_selected: false,
            min_age: None,
        },
        content: Content::Folders {
            folders: Folders::Trees(&[(Base::LocalAppData, r"pip\cache")]),
        },
    },
    Recipe {
        target: Metadata {
            id: "go-build-cache",
            name: "Go build cache",
            category: "Developer",
            default_selected: false,
            min_age: None,
        },
        content: Content::Folders {
            folders: Folders::Trees(&[(Base::LocalAppData, "go-build")]),
        },
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
            assert!(
                TARGETS[ix + 1..]
                    .iter()
                    .all(|other| other.target.id != target.target.id)
            );
        }
    }
}
