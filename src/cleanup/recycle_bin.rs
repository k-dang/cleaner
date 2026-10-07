//! The Recycle Bin Target. It uses only the shell's per-drive query and empty
//! calls and never opens `$Recycle.Bin` itself.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Storage::FileSystem::{GetLogicalDrives, GetVolumeNameForVolumeMountPointW};
use windows::Win32::UI::Shell::{
    SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW, SHQUERYRBINFO,
    SHQueryRecycleBinW,
};

use super::win::{classify, drive_root, is_fixed_drive, os_error, with_com};
use cleaner_core::results::{CleanResult, CleanStatus, Problem, ScanResult};

/// A drive whose Recycle Bin a Scan covered: its letter and the volume mounted
/// there, as a volume GUID path such as `\\?\Volume{...}\`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    pub letter: char,
    pub volume: String,
}

/// One drive's Recycle Bin, as the shell reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bin {
    pub bytes: u64,
    pub items: u64,
}

/// The shell calls the Recycle Bin Target makes. Tests substitute a fake.
pub trait Shell {
    /// Letters of the mounted local fixed drives, in order.
    fn fixed_drives(&self) -> io::Result<Vec<char>>;
    /// The volume GUID path of the volume currently mounted at `letter`.
    fn volume(&self, letter: char) -> io::Result<String>;
    /// The current account's Recycle Bin on `drive`.
    fn query(&self, drive: char) -> io::Result<Bin>;
    /// Empties the current account's Recycle Bin on `drive`.
    fn empty(&self, drive: char) -> io::Result<()>;
}

/// The Windows shell. Every call names one drive's root; an empty root would
/// cover every drive.
pub struct SystemShell;

impl Shell for SystemShell {
    fn fixed_drives(&self) -> io::Result<Vec<char>> {
        // SAFETY: GetLogicalDrives only returns a bit mask.
        let mask = unsafe { GetLogicalDrives() };
        if mask == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(('A'..='Z')
            .zip(0..)
            .filter(|&(_, bit)| mask & (1 << bit) != 0)
            .map(|(letter, _)| letter)
            .filter(|&letter| is_fixed_drive(&drive_root(letter)))
            .collect())
    }

    fn volume(&self, letter: char) -> io::Result<String> {
        // A volume GUID path is 49 characters plus the terminator.
        let mut name = [0u16; 50];
        // SAFETY: the root is terminated, and `name` is writable for its whole length.
        unsafe { GetVolumeNameForVolumeMountPointW(&drive_root(letter), &mut name) }
            .map_err(os_error)?;
        let length = name
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(name.len());
        String::from_utf16(&name[..length])
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    fn query(&self, drive: char) -> io::Result<Bin> {
        with_com(|| {
            let mut info = SHQUERYRBINFO {
                cbSize: size_of::<SHQUERYRBINFO>().try_into().unwrap(),
                ..Default::default()
            };
            // SAFETY: the root is terminated, and `info` is a sized, writable SHQUERYRBINFO.
            unsafe { SHQueryRecycleBinW(&drive_root(drive), &mut info) }.map_err(os_error)?;
            let count = |value: i64| {
                u64::try_from(value).map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "negative Recycle Bin size")
                })
            };
            Ok(Bin {
                bytes: count(info.i64Size)?,
                items: count(info.i64NumItems)?,
            })
        })
    }

    fn empty(&self, drive: char) -> io::Result<()> {
        let flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND;
        with_com(|| {
            // SAFETY: the root is terminated, and no owner window is needed without UI.
            unsafe { SHEmptyRecycleBinW(None, &drive_root(drive), flags) }.map_err(os_error)
        })
    }
}

/// Scans the current account's Recycle Bin on every mounted local fixed drive.
/// Also returns the drives it covered, which a later Clean reuses.
pub fn scan(shell: &dyn Shell, stop: &AtomicBool) -> (ScanResult, Vec<Drive>) {
    let letters = match shell.fixed_drives() {
        Ok(letters) => letters,
        Err(error) => {
            let problem = classify(&error);
            return (ScanResult::Failed { problem }, Vec::new());
        }
    };
    let mut bytes = 0;
    let mut drives = Vec::new();
    let mut problem = None;
    for letter in letters {
        if stop.load(Ordering::Relaxed) {
            return (ScanResult::Stopped, drives);
        }
        let queried = shell
            .volume(letter)
            .and_then(|volume| Ok((volume, shell.query(letter)?)));
        match queried {
            Ok((volume, bin)) => {
                bytes += bin.bytes;
                drives.push(Drive { letter, volume });
            }
            Err(error) => {
                problem.get_or_insert(classify(&error));
            }
        }
    }
    let result = match problem {
        None => ScanResult::Complete { bytes },
        Some(problem) if !drives.is_empty() => ScanResult::Partial { bytes, problem },
        Some(problem) => ScanResult::Failed { problem },
    };
    (result, drives)
}

/// Empties the Recycle Bin on each of `drives`. The shell reports no per-file
/// results, so deleted bytes and skip counts stay unavailable.
pub fn clean(shell: &dyn Shell, drives: &[Drive], stop: &AtomicBool) -> CleanResult {
    // A shell call cannot be cancelled, so stop is checked before each one.
    let stopping = || stop.load(Ordering::Relaxed);
    let mut emptied = false;
    let mut stopped = false;
    let mut problem = None;
    for drive in drives {
        stopped = stopping();
        if stopped {
            break;
        }
        let bin = shell.query(drive.letter);
        // The shell names a bin only by drive letter, so a letter remounted since
        // the Scan is skipped rather than emptying a bin the Scan never covered.
        // Checking after the query leaves only a stop check before the empty call.
        match shell.volume(drive.letter) {
            Ok(volume) if volume == drive.volume => {}
            Ok(_) => {
                problem.get_or_insert(Problem::LocationChanged);
                continue;
            }
            Err(error) => {
                problem.get_or_insert(classify(&error));
                continue;
            }
        }
        match bin {
            Err(error) => {
                problem.get_or_insert(classify(&error));
                continue;
            }
            // The shell can report an error for emptying an already-empty bin.
            Ok(bin) if bin.items == 0 => continue,
            Ok(_) => {}
        }
        stopped = stopping();
        if stopped {
            break;
        }
        match shell.empty(drive.letter) {
            Ok(()) => emptied = true,
            Err(error) => {
                problem.get_or_insert(classify(&error));
            }
        }
    }
    let status = match problem {
        _ if stopped => CleanStatus::Stopped,
        None => CleanStatus::Complete,
        Some(_) if emptied => CleanStatus::Partial,
        Some(_) => CleanStatus::Failed,
    };
    CleanResult {
        status,
        deleted_bytes: None,
        skipped: Vec::new(),
        coverage_problem: problem,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Mutex};

    const ACCESS_DENIED: i32 = 5;
    const NOT_READY: i32 = 21;

    /// A shell whose fixed drives are the keys of `bins`. Errors are Win32 codes.
    #[derive(Default)]
    pub(crate) struct FakeShell {
        pub bins: Mutex<BTreeMap<char, Result<Bin, i32>>>,
        /// The volume mounted at each letter; `volume_of(letter)` unless remounted.
        pub volumes: Mutex<HashMap<char, String>>,
        pub empty_errors: HashMap<char, i32>,
        /// Every call in order, such as `('empty', 'C')`.
        pub calls: Mutex<Vec<(&'static str, char)>>,
        /// Set while an `empty` call is in progress, as closing the window would.
        pub stop_during_empty: Option<Arc<AtomicBool>>,
        /// Mounts another volume at this letter while its bin is queried.
        pub remount_during_query: Option<char>,
    }

    impl FakeShell {
        pub fn new(bins: &[(char, Result<Bin, i32>)]) -> Self {
            Self {
                bins: Mutex::new(bins.iter().copied().collect()),
                ..Self::default()
            }
        }
    }

    impl Shell for FakeShell {
        fn fixed_drives(&self) -> io::Result<Vec<char>> {
            Ok(self.bins.lock().unwrap().keys().copied().collect())
        }

        fn volume(&self, letter: char) -> io::Result<String> {
            let volumes = self.volumes.lock().unwrap();
            Ok(volumes
                .get(&letter)
                .cloned()
                .unwrap_or_else(|| volume_of(letter)))
        }

        fn query(&self, drive: char) -> io::Result<Bin> {
            self.calls.lock().unwrap().push(("query", drive));
            if self.remount_during_query == Some(drive) {
                self.volumes.lock().unwrap().insert(drive, volume_of('X'));
            }
            self.bins.lock().unwrap()[&drive].map_err(io::Error::from_raw_os_error)
        }

        fn empty(&self, drive: char) -> io::Result<()> {
            self.calls.lock().unwrap().push(("empty", drive));
            if let Some(stop) = &self.stop_during_empty {
                stop.store(true, Ordering::Release);
            }
            if let Some(&code) = self.empty_errors.get(&drive) {
                return Err(io::Error::from_raw_os_error(code));
            }
            self.bins
                .lock()
                .unwrap()
                .insert(drive, Ok(Bin { bytes: 0, items: 0 }));
            Ok(())
        }
    }

    pub(crate) fn bin(bytes: u64, items: u64) -> Result<Bin, i32> {
        Ok(Bin { bytes, items })
    }

    fn volume_of(letter: char) -> String {
        format!(r"\\?\Volume{{{letter}}}\")
    }

    /// A drive as a Scan of `FakeShell` records it.
    pub(crate) fn drive(letter: char) -> Drive {
        Drive {
            letter,
            volume: volume_of(letter),
        }
    }

    #[test]
    fn scan_totals_every_fixed_drive_and_an_empty_bin_is_complete() {
        let stop = AtomicBool::new(false);
        let shell = FakeShell::new(&[('C', bin(300, 2)), ('D', bin(0, 0))]);
        assert_eq!(
            scan(&shell, &stop),
            (
                ScanResult::Complete { bytes: 300 },
                vec![drive('C'), drive('D')]
            )
        );
        let empty = FakeShell::new(&[('C', bin(0, 0))]);
        assert_eq!(
            scan(&empty, &stop),
            (ScanResult::Complete { bytes: 0 }, vec![drive('C')])
        );
    }

    #[test]
    fn clean_empties_only_the_scanned_drives_and_reports_no_counts() {
        let stop = AtomicBool::new(false);
        let shell = FakeShell::new(&[('C', bin(300, 2)), ('D', bin(0, 0)), ('F', bin(9, 1))]);
        let (_, drives) = scan(&shell, &stop);
        shell.bins.lock().unwrap().insert('E', bin(5, 1));
        shell.calls.lock().unwrap().clear();
        assert_eq!(
            clean(&shell, &drives, &stop),
            CleanResult {
                status: CleanStatus::Complete,
                deleted_bytes: None,
                skipped: vec![],
                coverage_problem: None,
            }
        );
        // An already-empty bin is left alone, and E appeared after the Scan.
        assert_eq!(
            *shell.calls.lock().unwrap(),
            [
                ("query", 'C'),
                ("empty", 'C'),
                ("query", 'D'),
                ("query", 'F'),
                ("empty", 'F')
            ]
        );
    }

    #[test]
    fn clean_is_partial_when_another_drive_was_emptied_and_failed_otherwise() {
        let stop = AtomicBool::new(false);
        let mut shell = FakeShell::new(&[('C', bin(300, 2)), ('D', bin(7, 1))]);
        shell.empty_errors.insert('D', ACCESS_DENIED);
        let partial = clean(&shell, &[drive('C'), drive('D')], &stop);
        assert_eq!(
            (
                partial.status,
                partial.coverage_problem,
                partial.deleted_bytes
            ),
            (CleanStatus::Partial, Some(Problem::AccessDenied), None)
        );
        // C is now empty, so D's failure means no drive was emptied.
        let failed = clean(&shell, &[drive('C'), drive('D')], &stop);
        assert_eq!(
            (failed.status, failed.coverage_problem, failed.deleted_bytes),
            (CleanStatus::Failed, Some(Problem::AccessDenied), None)
        );
        let unreadable = FakeShell::new(&[('C', Err(NOT_READY))]);
        assert_eq!(
            clean(&unreadable, &[drive('C')], &stop).status,
            CleanStatus::Failed
        );
    }

    #[test]
    fn clean_does_not_empty_a_drive_letter_remounted_since_the_scan() {
        let stop = AtomicBool::new(false);
        let mut shell = FakeShell::new(&[('C', bin(300, 2)), ('D', bin(7, 1))]);
        let (_, drives) = scan(&shell, &stop);
        shell.remount_during_query = Some('D');
        shell.calls.lock().unwrap().clear();
        let result = clean(&shell, &drives, &stop);
        assert_eq!(
            (result.status, result.coverage_problem),
            (CleanStatus::Partial, Some(Problem::LocationChanged))
        );
        assert_eq!(
            *shell.calls.lock().unwrap(),
            [("query", 'C'), ("empty", 'C'), ("query", 'D')]
        );
    }

    #[test]
    fn stop_during_an_empty_call_starts_no_further_shell_call() {
        let stop = Arc::new(AtomicBool::new(false));
        let mut shell = FakeShell::new(&[('C', bin(300, 2)), ('D', bin(7, 1))]);
        shell.stop_during_empty = Some(stop.clone());
        let result = clean(&shell, &[drive('C'), drive('D')], &stop);
        assert_eq!(result.status, CleanStatus::Stopped);
        assert_eq!(result.deleted_bytes, None);
        assert_eq!(
            *shell.calls.lock().unwrap(),
            [("query", 'C'), ("empty", 'C')]
        );
        assert_eq!(scan(&shell, &stop).0, ScanResult::Stopped);
        assert_eq!(shell.calls.lock().unwrap().len(), 2);
    }

    #[test]
    fn scan_reports_failed_drives_without_counting_them() {
        let stop = AtomicBool::new(false);
        let shell = FakeShell::new(&[('C', bin(300, 2)), ('D', Err(ACCESS_DENIED))]);
        assert_eq!(
            scan(&shell, &stop).0,
            ScanResult::Partial {
                bytes: 300,
                problem: Problem::AccessDenied
            }
        );
        let shell = FakeShell::new(&[('C', Err(NOT_READY)), ('D', Err(ACCESS_DENIED))]);
        assert_eq!(
            scan(&shell, &stop).0,
            ScanResult::Failed {
                problem: Problem::Other
            }
        );
    }
}
