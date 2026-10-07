//! Scan and Clean of the built-in Targets, one at a time on the calling worker.
//! Folder Targets are walked through verified directory handles; the Recycle Bin
//! uses the shell.

use std::io;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use windows::Win32::UI::Shell::{FOLDERID_LocalAppData, FOLDERID_ProgramData, FOLDERID_Windows};
use windows::core::GUID;

use crate::targets::{self, Base, Content, Recipe};
use cleaner_core::controller::CleanTarget;
use cleaner_core::results::{CleanResult, CleanStatus, Problem};
use cleaner_core::targets::TargetId;

pub mod recycle_bin;
#[cfg(test)]
mod tests;
mod walk;
mod win;

use recycle_bin::{Drive, Shell};
use walk::{clean_folders, scan_folders};
use win::{RootHandles, known_folder, root_handle, root_problem};

/// Evidence produced by a Windows Scan and returned unchanged by the portable controller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Snapshot {
    Folders,
    RecycleBin(Vec<Drive>),
}

pub type Event = cleaner_core::results::Event<Snapshot>;

/// The known folders Target paths are relative to. Production roots come from
/// Windows known-folder APIs. Tests use fixture roots.
pub struct Roots {
    local_app_data: Result<PathBuf, Problem>,
    win_dir: Result<PathBuf, Problem>,
    program_data: Result<PathBuf, Problem>,
}

impl Roots {
    pub fn system() -> Self {
        Self::resolve(known_folder)
    }

    /// A root that fails to resolve fails only the Targets that use it.
    fn resolve(mut folder: impl FnMut(&GUID) -> io::Result<PathBuf>) -> Self {
        let mut root = |id| {
            folder(id)
                .and_then(validate_folder)
                .map_err(|error| root_problem(&error))
        };
        Self {
            local_app_data: root(&FOLDERID_LocalAppData),
            win_dir: root(&FOLDERID_Windows),
            program_data: root(&FOLDERID_ProgramData),
        }
    }

    fn base(&self, base: Base) -> Result<&Path, Problem> {
        let root = match base {
            Base::LocalAppData => &self.local_app_data,
            Base::WinDir => &self.win_dir,
            Base::ProgramData => &self.program_data,
        };
        root.as_deref().map_err(|problem| *problem)
    }

    /// Opens a Target folder. `None` means it is confirmed absent.
    fn open(&self, base: Base, path: &str) -> Result<Option<RootHandles>, Problem> {
        root_handle(&self.base(base)?.join(path)).map_err(|error| root_problem(&error))
    }
}

/// Resolve and validate only the requested folder; unrelated roots cannot block it.
pub(crate) fn validated_known_folder(id: &GUID) -> io::Result<PathBuf> {
    known_folder(id).and_then(validate_folder)
}

fn validate_folder(path: PathBuf) -> io::Result<PathBuf> {
    if root_handle(&path)?.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "known folder is absent",
        ));
    }
    Ok(path)
}

fn target(id: TargetId) -> &'static Recipe {
    targets::find(id).expect("workers receive only built-in Target IDs")
}

/// Scans `targets` in order, reporting each result as it finishes, then `Finished`.
pub fn scan_targets(
    targets: &[TargetId],
    roots: &Roots,
    shell: &dyn Shell,
    time: SystemTime,
    stop: &AtomicBool,
    report: &mut dyn FnMut(Event),
) {
    for &id in targets {
        if stop.load(Ordering::Acquire) {
            break;
        }
        report(Event::Scanning(id));
        report(match &target(id).content {
            Content::Folders { folders } => {
                let result = scan_folders(
                    folders,
                    target(id).target.min_age,
                    roots,
                    time,
                    stop,
                    &mut |_| {},
                );
                Event::Scanned(id, result, Snapshot::Folders)
            }
            Content::RecycleBin => {
                let (result, drives) = recycle_bin::scan(shell, stop);
                Event::Scanned(id, result, Snapshot::RecycleBin(drives))
            }
        });
    }
    report(Event::Finished);
}

/// Cleans ready Targets using their latest Scan snapshots, reporting each result.
/// The Recycle Bin is emptied only on the volumes captured by its Scan.
pub fn clean_targets(
    targets: &[CleanTarget<Snapshot>],
    roots: &Roots,
    shell: &dyn Shell,
    time: SystemTime,
    stop: &AtomicBool,
    report: &mut dyn FnMut(Event),
) {
    for scanned in targets {
        if stop.load(Ordering::Acquire) {
            break;
        }
        let id = scanned.id;
        report(Event::Cleaning(id));
        let result = match (&target(id).content, &scanned.snapshot) {
            (Content::Folders { folders }, Snapshot::Folders) => clean_folders(
                folders,
                target(id).target.min_age,
                roots,
                time,
                stop,
                &mut |_| {},
            ),
            (Content::RecycleBin, Snapshot::RecycleBin(drives)) => {
                recycle_bin::clean(shell, drives, stop)
            }
            _ => CleanResult {
                status: CleanStatus::Failed,
                deleted_bytes: Some(0),
                skipped: Vec::new(),
                coverage_problem: Some(Problem::Metadata),
            },
        };
        report(Event::Cleaned(id, result));
    }
    report(Event::Finished);
}
