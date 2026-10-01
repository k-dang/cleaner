//! Handle-relative traversal of a folder Target's folders for Scan and Clean.
//! All child opens are relative to a verified parent handle. A name seen in a
//! directory listing is never later resolved through an absolute path.

use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::{align_of, offset_of, size_of};
use std::os::windows::ffi::OsStringExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{ERROR_DIR_NOT_EMPTY, ERROR_NO_MORE_FILES, WIN32_ERROR};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_TAG_INFO, FILE_BASIC_INFO, FILE_ID_BOTH_DIR_INFO,
    FILE_ID_INFO, FILE_STANDARD_INFO, FileAttributeTagInfo, FileBasicInfo, FileIdBothDirectoryInfo,
    FileIdInfo, FileStandardInfo, GetFileInformationByHandleEx,
};

use crate::results::{CleanResult, CleanStatus, Problem, ScanResult, add_count};
use crate::targets::{self, Base, Folders};

use super::Roots;
use super::win::{
    OwnedHandle, RootHandles, classify, delete, info, is_link, is_reparse, open_dir, os_error,
    relative_open, root_problem, win32_code,
};

/// The FILETIME a file must be modified strictly before to be `min_age` old at `time`.
fn cutoff(time: SystemTime, min_age: Duration) -> i64 {
    let duration = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .saturating_sub(min_age);
    // Windows FILETIME counts 100 ns intervals from 1601-01-01.
    let ticks = duration.as_nanos() / 100 + 116_444_736_000_000_000;
    i64::try_from(ticks).unwrap_or(i64::MAX)
}

/// The logical size of an eligible file, or `None` if it is newer than `cutoff`.
/// A link counts as 0 bytes, since removing it frees nothing it points to.
fn eligible(handle: &OwnedHandle, link: bool, cutoff: Option<i64>) -> io::Result<Option<u64>> {
    if let Some(cutoff) = cutoff {
        let basic: FILE_BASIC_INFO = info(handle, FileBasicInfo)?;
        if basic.LastWriteTime <= 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file time unavailable",
            ));
        }
        if basic.LastWriteTime >= cutoff {
            return Ok(None);
        }
    }
    if link {
        return Ok(Some(0));
    }
    let standard: FILE_STANDARD_INFO = info(handle, FileStandardInfo)?;
    Ok(Some(u64::try_from(standard.EndOfFile).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "file size unavailable")
    })?))
}

struct Entry {
    name: OsString,
    directory: bool,
}

fn entries(handle: &OwnedHandle, stop: &AtomicBool) -> io::Result<Vec<Entry>> {
    let mut found = Vec::new();
    let mut buffer = [0u64; 8192];
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err(io::ErrorKind::Interrupted.into());
        }
        // SAFETY: aligned writable buffer. The kernel writes complete entries.
        let result = unsafe {
            GetFileInformationByHandleEx(
                handle.0,
                FileIdBothDirectoryInfo,
                buffer.as_mut_ptr().cast(),
                size_of_val(&buffer)
                    .try_into()
                    .expect("directory buffer size"),
            )
        };
        if let Err(error) = result {
            if WIN32_ERROR::from_error(&error) == Some(ERROR_NO_MORE_FILES) {
                break;
            }
            return Err(os_error(error));
        }
        let mut offset = 0usize;
        loop {
            if offset + size_of::<FILE_ID_BOTH_DIR_INFO>() > size_of_val(&buffer) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid directory record",
                ));
            }
            // SAFETY: offset is checked and each record is aligned to 8 bytes.
            let record = unsafe {
                &*(buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_BOTH_DIR_INFO>())
            };
            let name_offset = offset + offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
            let name_len = record.FileNameLength as usize;
            let next = record.NextEntryOffset as usize;
            if next != 0
                && (next < size_of::<FILE_ID_BOTH_DIR_INFO>()
                    || !next.is_multiple_of(align_of::<FILE_ID_BOTH_DIR_INFO>())
                    || offset + next > size_of_val(&buffer))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid directory offset",
                ));
            }
            let entry_end = if next == 0 {
                size_of_val(&buffer)
            } else {
                offset + next
            };
            if !name_len.is_multiple_of(2) || name_offset + name_len > entry_end {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid directory name",
                ));
            }
            // SAFETY: name length and bounds were checked above.
            let units = unsafe {
                std::slice::from_raw_parts(
                    buffer.as_ptr().cast::<u8>().add(name_offset).cast::<u16>(),
                    name_len / 2,
                )
            };
            let name = OsString::from_wide(units);
            if name != "." && name != ".." {
                found.push(Entry {
                    name,
                    directory: record.FileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0,
                });
            }
            if next == 0 {
                break;
            }
            offset += next;
        }
    }
    Ok(found)
}

/// What a walk did with one child of a directory.
enum Removal {
    Removed,
    Kept,
    /// It vanished before the walk could remove it, so it neither blocks nor
    /// counts toward pruning its parent.
    Gone,
}

/// One Scan or Clean walk over a Target: its settings and what it found so far.
struct Walk<'a> {
    clean: bool,
    /// Files modified at or after this FILETIME are kept. `None` without a Minimum age.
    cutoff: Option<i64>,
    stop: &'a AtomicBool,
    /// Test hook, called before each child is opened or reopened.
    before_open: &'a mut dyn FnMut(&OsStr),
    bytes: u64,
    /// True once Clean removed any file, link, or emptied directory.
    deleted_any: bool,
    /// True once the contents of any Target folder were listed.
    visited: bool,
    problem: Option<Problem>,
    skipped: Vec<(Problem, u64)>,
    stopped: bool,
}

impl<'a> Walk<'a> {
    fn new(
        clean: bool,
        min_age: Option<Duration>,
        time: SystemTime,
        stop: &'a AtomicBool,
        before_open: &'a mut dyn FnMut(&OsStr),
    ) -> Self {
        Self {
            clean,
            cutoff: min_age.map(|age| cutoff(time, age)),
            stop,
            before_open,
            bytes: 0,
            deleted_any: false,
            visited: false,
            problem: None,
            skipped: Vec::new(),
            stopped: false,
        }
    }

    /// Records a problem. A file that Clean skips for a reason other than
    /// unavailable metadata counts only in `skipped`, not as a coverage problem.
    fn problem(&mut self, problem: Problem, file: bool) {
        if !file || !self.clean || problem == Problem::Metadata {
            self.problem.get_or_insert(problem);
        }
        if file {
            add_count(&mut self.skipped, problem, 1);
        }
    }

    fn rejected(&self) -> bool {
        self.problem.is_some() || !self.skipped.is_empty()
    }

    /// True once a stop was requested; the walk then returns without further work.
    fn stopping(&mut self) -> bool {
        self.stopped |= self.stop.load(Ordering::Relaxed);
        self.stopped
    }

    /// Processes every folder of a Target. An absent folder adds nothing; one
    /// that cannot be opened safely is recorded as a problem.
    fn target(&mut self, folders: &Folders, roots: &Roots) {
        if self.stopping() {
            return;
        }
        match *folders {
            Folders::Trees(folders) => {
                for &(base, path) in folders {
                    if self.stopping() {
                        return;
                    }
                    if let Some(root) = self.open_root(roots, base, path) {
                        self.walk(root.target());
                    }
                }
            }
            Folders::Files {
                base,
                path,
                prefix,
                suffix,
            } => {
                if let Some(root) = self.open_root(roots, base, path) {
                    self.files(root.target(), prefix, suffix);
                }
            }
            Folders::Profiles { base, path, caches } => {
                if let Some(root) = self.open_root(roots, base, path) {
                    self.profiles(root.target(), caches);
                }
            }
        }
    }

    fn open_root(&mut self, roots: &Roots, base: Base, path: &str) -> Option<RootHandles> {
        roots.open(base, path).unwrap_or_else(|problem| {
            self.problem(problem, false);
            None
        })
    }

    /// Opens an ordinary child directory, recording why it could not be opened.
    fn child_dir(&mut self, parent: &OwnedHandle, name: &OsStr) -> Option<OwnedHandle> {
        open_dir(parent, name).unwrap_or_else(|error| {
            self.problem(root_problem(&error), false);
            None
        })
    }

    /// Lists a directory. `None` when it could not be listed or a stop was requested.
    fn list(&mut self, directory: &OwnedHandle) -> Option<Vec<Entry>> {
        match entries(directory, self.stop) {
            Ok(children) => Some(children),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                self.stopped = true;
                None
            }
            Err(error) => {
                self.problem(classify(&error), false);
                None
            }
        }
    }

    /// Opens a listed child without following a link. `Ok(None)` means it vanished;
    /// `Err` means the failure was recorded.
    fn open_child(
        &mut self,
        directory: &OwnedHandle,
        child: &Entry,
        delete: bool,
    ) -> Result<Option<(OwnedHandle, FILE_ATTRIBUTE_TAG_INFO)>, ()> {
        let opened = relative_open(directory, &child.name, child.directory, delete)
            .and_then(|handle| Ok((info(&handle, FileAttributeTagInfo)?, handle)));
        match opened {
            Ok((tag, handle)) => Ok(Some((handle, tag))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => {
                self.problem(classify(&error), !child.directory);
                Err(())
            }
        }
    }

    /// Walks the fixed cache folders inside each Chromium profile directory of
    /// `parent`. Nothing else in a profile is visited.
    fn profiles(&mut self, parent: &OwnedHandle, caches: &[&str]) {
        let Some(children) = self.list(parent) else {
            return;
        };
        for child in children {
            let profile = child
                .name
                .to_str()
                .is_some_and(targets::is_chromium_profile);
            if !child.directory || !profile {
                continue;
            }
            if self.stopping() {
                return;
            }
            (self.before_open)(&child.name);
            let Some(profile) = self.child_dir(parent, &child.name) else {
                continue;
            };
            for cache in caches {
                if self.stopping() {
                    return;
                }
                if let Some(folder) = self.child_dir(&profile, OsStr::new(cache)) {
                    self.walk(&folder);
                }
            }
        }
    }

    /// Handles the immediate files of `directory` named `prefix*suffix`, ignoring
    /// every other entry and all descendants.
    fn files(&mut self, directory: &OwnedHandle, prefix: &str, suffix: &str) {
        let Some(children) = self.list(directory) else {
            return;
        };
        self.visited = true;
        for child in children {
            if child.directory || !matches(&child.name, prefix, suffix) {
                continue;
            }
            if self.stopping() {
                return;
            }
            (self.before_open)(&child.name);
            if self.stopping() {
                return;
            }
            if let Ok(Some((handle, tag))) = self.open_child(directory, &child, false) {
                self.file(directory, &child, handle, &tag);
            }
        }
    }

    /// Returns true when cleanup emptied `directory` and it can be pruned.
    fn walk(&mut self, directory: &OwnedHandle) -> bool {
        if self.stopping() {
            return false;
        }
        let Some(children) = self.list(directory) else {
            return false;
        };
        self.visited = true;
        let mut empty = true;
        let mut removed = false;
        for child in children {
            if self.stopping() {
                return false;
            }
            (self.before_open)(&child.name);
            if self.stopping() {
                return false;
            }
            let outcome = match self.open_child(directory, &child, false) {
                Ok(None) => Removal::Gone,
                Err(()) => Removal::Kept,
                Ok(Some((handle, tag))) if child.directory && !is_reparse(&tag) => {
                    let child_empty = self.walk(&handle);
                    if self.stopping() {
                        return false;
                    }
                    if self.clean && child_empty {
                        // Only empty descendants are removed. A Target folder is never passed here.
                        self.prune(directory, &child, handle)
                    } else {
                        Removal::Kept
                    }
                }
                Ok(Some((handle, tag))) => self.file(directory, &child, handle, &tag),
            };
            match outcome {
                Removal::Removed => removed = true,
                Removal::Kept => empty = false,
                Removal::Gone => {}
            }
        }
        empty && removed
    }

    /// Releases a child's inspection handle and reopens it with DELETE access. The
    /// inspection handle denies delete sharing, so it cannot request DELETE itself.
    /// Callers recheck the reopened child, since its name may have been replaced.
    fn reopen_for_delete(
        &mut self,
        directory: &OwnedHandle,
        child: &Entry,
        handle: OwnedHandle,
    ) -> Result<(OwnedHandle, FILE_ATTRIBUTE_TAG_INFO), Removal> {
        drop(handle);
        (self.before_open)(&child.name);
        if self.stopping() {
            return Err(Removal::Kept);
        }
        match self.open_child(directory, child, true) {
            Ok(Some(opened)) => Ok(opened),
            Ok(None) => Err(Removal::Gone),
            Err(()) => Err(Removal::Kept),
        }
    }

    /// Deletes a directory this walk emptied. Traversal holds no DELETE access,
    /// so a holder that denies delete sharing cannot block cleaning its contents.
    /// The reopened directory must have the file ID of the one walked.
    fn prune(&mut self, parent: &OwnedHandle, child: &Entry, handle: OwnedHandle) -> Removal {
        let Ok(identity) = info::<FILE_ID_INFO>(&handle, FileIdInfo) else {
            self.problem(Problem::Metadata, false);
            return Removal::Kept;
        };
        let (handle, tag) = match self.reopen_for_delete(parent, child, handle) {
            Ok(opened) => opened,
            Err(outcome) => return outcome,
        };
        let Ok(current) = info::<FILE_ID_INFO>(&handle, FileIdInfo) else {
            self.problem(Problem::Metadata, false);
            return Removal::Kept;
        };
        if is_reparse(&tag) || current != identity {
            self.problem(Problem::Redirected, false);
            return Removal::Kept;
        }
        if self.stopping() {
            return Removal::Kept;
        }
        match delete(&handle) {
            Ok(()) => {
                self.deleted_any = true;
                Removal::Removed
            }
            Err(error) => {
                if win32_code(&error) != Some(ERROR_DIR_NOT_EMPTY) {
                    self.problem(classify(&error), false);
                }
                Removal::Kept
            }
        }
    }

    /// Checks both the reparse tag and Minimum age on the opened file itself.
    fn file_bytes(&mut self, handle: &OwnedHandle, tag: &FILE_ATTRIBUTE_TAG_INFO) -> Option<u64> {
        if is_reparse(tag) && !is_link(tag) {
            self.problem(Problem::Redirected, false);
            return None;
        }
        match eligible(handle, is_link(tag), self.cutoff) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.problem(Problem::Metadata, true);
                None
            }
        }
    }

    /// Scans or cleans one file or link.
    fn file(
        &mut self,
        directory: &OwnedHandle,
        child: &Entry,
        handle: OwnedHandle,
        tag: &FILE_ATTRIBUTE_TAG_INFO,
    ) -> Removal {
        let Some(bytes) = self.file_bytes(&handle, tag) else {
            return Removal::Kept;
        };
        if !self.clean {
            self.bytes = self.bytes.saturating_add(bytes);
            return Removal::Kept;
        }
        let (handle, tag) = match self.reopen_for_delete(directory, child, handle) {
            Ok(opened) => opened,
            Err(outcome) => return outcome,
        };
        // Recheck the reopened file or link: it may have changed.
        let Some(bytes) = self.file_bytes(&handle, &tag) else {
            return Removal::Kept;
        };
        if self.stopping() {
            return Removal::Kept;
        }
        match delete(&handle) {
            Ok(()) => {
                self.bytes = self.bytes.saturating_add(bytes);
                self.deleted_any = true;
                Removal::Removed
            }
            Err(error) => {
                self.problem(classify(&error), true);
                Removal::Kept
            }
        }
    }
}

/// True when `name` is `prefix*suffix`, ignoring ASCII case.
fn matches(name: &OsStr, prefix: &str, suffix: &str) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    name.len() >= prefix.len() + suffix.len()
        && name
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        && name
            .get(name.len() - suffix.len()..)
            .is_some_and(|end| end.eq_ignore_ascii_case(suffix))
}

/// Scans a folder Target's folders at a fixed operation time.
pub(super) fn scan_folders(
    folders: &Folders,
    min_age: Option<Duration>,
    roots: &Roots,
    time: SystemTime,
    stop: &AtomicBool,
    before_open: &mut dyn FnMut(&OsStr),
) -> ScanResult {
    let mut walk = Walk::new(false, min_age, time, stop, before_open);
    walk.target(folders, roots);
    if walk.stopped {
        ScanResult::Stopped
    } else if let Some(problem) = walk.problem {
        if walk.visited {
            ScanResult::Partial {
                bytes: walk.bytes,
                problem,
            }
        } else {
            ScanResult::Failed { problem }
        }
    } else if walk.visited {
        ScanResult::Complete { bytes: walk.bytes }
    } else {
        ScanResult::NotPresent
    }
}

/// Cleans a folder Target's folders. Scan results never authorize a deletion.
pub(super) fn clean_folders(
    folders: &Folders,
    min_age: Option<Duration>,
    roots: &Roots,
    time: SystemTime,
    stop: &AtomicBool,
    before_open: &mut dyn FnMut(&OsStr),
) -> CleanResult {
    let mut walk = Walk::new(true, min_age, time, stop, before_open);
    walk.target(folders, roots);
    let status = if walk.stopped {
        CleanStatus::Stopped
    } else if walk.rejected() {
        if walk.deleted_any {
            CleanStatus::Partial
        } else {
            CleanStatus::Failed
        }
    } else {
        CleanStatus::Complete
    };
    CleanResult {
        status,
        deleted_bytes: Some(walk.bytes),
        skipped: walk.skipped,
        coverage_problem: walk.problem,
    }
}
