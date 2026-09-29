//! Handle-relative traversal for the built-in folder Targets.
//! All child opens are relative to a verified parent handle. A name seen in a
//! directory listing is never later resolved through an absolute path.

use std::ffi::{OsStr, OsString};
use std::io;
use std::mem::{align_of, offset_of, size_of};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_DIR_NOT_EMPTY, ERROR_LOCK_VIOLATION,
    ERROR_NO_MORE_FILES, ERROR_SHARING_VIOLATION, HANDLE, OBJ_CASE_INSENSITIVE, RPC_E_CHANGED_MODE,
    RtlNtStatusToDosError, UNICODE_STRING, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_ATTRIBUTE_TAG_INFO, FILE_BASIC_INFO, FILE_DISPOSITION_FLAG_DELETE,
    FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_ID_BOTH_DIR_INFO, FILE_INFO_BY_HANDLE_CLASS,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FILE_STANDARD_INFO, FileAttributeTagInfo, FileBasicInfo, FileDispositionInfoEx,
    FileIdBothDirectoryInfo, FileStandardInfo, GetDriveTypeW, GetFileInformationByHandleEx,
    OPEN_EXISTING, SYNCHRONIZE, SetFileInformationByHandle,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::IO::IO_STATUS_BLOCK;
use windows::Win32::System::SystemServices::{IO_REPARSE_TAG_MOUNT_POINT, IO_REPARSE_TAG_SYMLINK};
use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_ProgramData, FOLDERID_Windows, KF_FLAG_DONT_VERIFY,
    SHGetKnownFolderPath,
};
use windows::core::{GUID, HSTRING, PWSTR};

use crate::results::{CleanResult, CleanStatus, Problem, ScanResult, add_count};
use crate::targets::{self, Base, Folders, Target, TargetId};

struct OwnedHandle(HANDLE);

struct RootHandles(Vec<OwnedHandle>);

impl RootHandles {
    fn target(&self) -> &OwnedHandle {
        self.0.last().expect("volume handle is always present")
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: this owns exactly one successful file open.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

// SAFETY: HANDLE is a kernel reference with no thread-affine state. Ownership
// stays unique and Drop closes it once.
unsafe impl Send for OwnedHandle {}

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

fn known_folder(id: &GUID) -> io::Result<PathBuf> {
    // SHGetKnownFolderPath requires COM on this thread. GPUI may already have
    // initialized it with a different apartment model.
    // SAFETY: no reserved pointer is supplied and this function balances a
    // successful initialization before returning.
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
        return Err(os_error(windows::core::Error::from(initialized)));
    }
    let owns_com = initialized.is_ok();
    let result = known_folder_initialized(id);
    if owns_com {
        // SAFETY: CoInitializeEx succeeded on this thread above.
        unsafe { CoUninitialize() };
    }
    result
}

fn known_folder_initialized(id: &GUID) -> io::Result<PathBuf> {
    // Resolve only: validate_folder rejects nonlocal paths before checking existence.
    // SAFETY: Shell returns a terminated path that remains allocated until freed below.
    let path = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY, None) }.map_err(os_error)?;
    // SAFETY: the shell returned a valid terminated UTF-16 buffer.
    let result = unsafe { path.to_string() }
        .map(PathBuf::from)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    // SAFETY: this is the pointer allocated by SHGetKnownFolderPath, freed once.
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(path.0.cast())) };
    result
}

/// Converts a `windows` error to an `io::Error` carrying the plain Win32 code, so
/// `raw_os_error` and `kind` see the same values as errors from std.
fn os_error(error: windows::core::Error) -> io::Error {
    match WIN32_ERROR::from_error(&error) {
        Some(code) => io::Error::from_raw_os_error(code.0.cast_signed()),
        None => io::Error::from(error),
    }
}

fn win32_code(error: &io::Error) -> Option<WIN32_ERROR> {
    error
        .raw_os_error()
        .map(|code| WIN32_ERROR(code.cast_unsigned()))
}

fn classify(error: &io::Error) -> Problem {
    match win32_code(error) {
        Some(ERROR_ACCESS_DENIED) => Problem::AccessDenied,
        Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) => Problem::SharingViolation,
        _ => Problem::Other,
    }
}

fn relative_open(
    parent: &OwnedHandle,
    name: &OsStr,
    directory: bool,
    delete: bool,
) -> io::Result<OwnedHandle> {
    let mut units: Vec<u16> = name.encode_wide().collect();
    if units.is_empty()
        || units
            .iter()
            .any(|&unit| unit == u16::from(b'\\') || unit == u16::from(b'/') || unit == 0)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid child name",
        ));
    }
    let length = u16::try_from(units.len() * 2)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "long name"))?;
    let name = UNICODE_STRING {
        Length: length,
        MaximumLength: length,
        Buffer: PWSTR(units.as_mut_ptr()),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>()
            .try_into()
            .expect("Win32 struct size"),
        RootDirectory: parent.0,
        ObjectName: &name,
        Attributes: OBJ_CASE_INSENSITIVE,
        ..Default::default()
    };
    let mut access = FILE_READ_ATTRIBUTES | SYNCHRONIZE;
    if directory {
        access |= FILE_LIST_DIRECTORY;
    }
    if delete {
        access |= DELETE;
    }
    let options = FILE_OPEN_REPARSE_POINT
        | FILE_SYNCHRONOUS_IO_NONALERT
        | if directory {
            FILE_DIRECTORY_FILE
        } else {
            FILE_NON_DIRECTORY_FILE
        };
    let mut handle = HANDLE::default();
    let mut status = IO_STATUS_BLOCK::default();
    // SAFETY: buffers and parent handle live for the synchronous call. The
    // returned handle is owned only on successful NTSTATUS.
    let code = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &attributes,
            &mut status,
            None,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            // Denying delete sharing pins every opened name against external rename
            // while it is inspected or deleted. An existing open without compatible
            // sharing is skipped instead of forcing access.
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            FILE_OPEN,
            options,
            None,
            0,
        )
    };
    if code.is_err() {
        // SAFETY: RtlNtStatusToDosError accepts any NTSTATUS value.
        let win32 = unsafe { RtlNtStatusToDosError(code) };
        return Err(io::Error::from_raw_os_error(win32.cast_signed()));
    }
    Ok(OwnedHandle(handle))
}

fn info<T: Default>(handle: &OwnedHandle, class: FILE_INFO_BY_HANDLE_CLASS) -> io::Result<T> {
    let mut value = T::default();
    // SAFETY: the typed buffer has the required size and remains live.
    unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            class,
            (&mut value as *mut T).cast(),
            size_of::<T>().try_into().expect("Win32 struct size"),
        )
        .map_err(os_error)?;
    }
    Ok(value)
}

fn is_reparse(tag: &FILE_ATTRIBUTE_TAG_INFO) -> bool {
    tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

/// A symlink or mount point: removable as a name without visiting its destination.
fn is_link(tag: &FILE_ATTRIBUTE_TAG_INFO) -> bool {
    is_reparse(tag)
        && (tag.ReparseTag == IO_REPARSE_TAG_SYMLINK
            || tag.ReparseTag == IO_REPARSE_TAG_MOUNT_POINT)
}

fn root_handle(path: &Path) -> io::Result<Option<RootHandles>> {
    let mut components = path.components();
    let drive = match (components.next(), components.next()) {
        (Some(Component::Prefix(prefix)), Some(Component::RootDir)) => match prefix.kind() {
            Prefix::Disk(letter) => Some(HSTRING::from(format!("{}:\\", char::from(letter)))),
            _ => None,
        },
        _ => None,
    }
    .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Target is not on a local drive",
        )
    })?;
    // SAFETY: `drive` is terminated and lives through the call.
    if unsafe { GetDriveTypeW(&drive) } != DRIVE_FIXED {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Target is not on a local fixed drive",
        ));
    }
    // SAFETY: flags open the drive root itself, without following a reparse point.
    let volume = OwnedHandle(unsafe {
        CreateFileW(
            &drive,
            (FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
        .map_err(os_error)?
    });
    let mut handles = vec![volume];
    for component in components {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid Target path",
            ));
        };
        match open_dir(handles.last().unwrap(), name)? {
            Some(current) => handles.push(current),
            None => return Ok(None),
        }
    }
    Ok(Some(RootHandles(handles)))
}

/// Opens an ordinary child directory. `None` means it is absent; a redirected
/// directory is an `InvalidData` error.
fn open_dir(parent: &OwnedHandle, name: &OsStr) -> io::Result<Option<OwnedHandle>> {
    let child = match relative_open(parent, name, true, false) {
        Ok(handle) => handle,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if is_reparse(&info(&child, FileAttributeTagInfo)?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "redirected folder",
        ));
    }
    Ok(Some(child))
}

fn root_problem(error: &io::Error) -> Problem {
    match error.kind() {
        io::ErrorKind::InvalidData => Problem::Redirected,
        io::ErrorKind::InvalidInput => Problem::NonLocal,
        _ => classify(error),
    }
}

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

fn delete(handle: &OwnedHandle) -> io::Result<()> {
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE,
    };
    // SAFETY: the handle was opened with DELETE and the struct lives for the call.
    unsafe {
        SetFileInformationByHandle(
            handle.0,
            FileDispositionInfoEx,
            (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>()
                .try_into()
                .expect("Win32 struct size"),
        )
        .map_err(os_error)
    }
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

/// One Scan or Clean walk over a Target: its settings and what it found so far.
struct Walk<'a> {
    clean: bool,
    /// Files modified at or after this FILETIME are kept. `None` without a Minimum age.
    cutoff: Option<i64>,
    stop: &'a AtomicBool,
    /// Test hook, called before each child is opened.
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
        target: &Target,
        time: SystemTime,
        stop: &'a AtomicBool,
        before_open: &'a mut dyn FnMut(&OsStr),
    ) -> Self {
        Self {
            clean,
            cutoff: target.min_age.map(|age| cutoff(time, age)),
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

    /// Processes every folder of `target`. An absent folder adds nothing; one
    /// that cannot be opened safely is recorded as a problem.
    fn target(&mut self, target: &Target, roots: &Roots) {
        if self.stopping() {
            return;
        }
        match target.folders {
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
                patterns,
            } => {
                if let Some(root) = self.open_root(roots, base, path) {
                    self.files(root.target(), patterns);
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

    /// Handles the immediate files of `directory` named by `patterns`, ignoring
    /// every other entry and all descendants.
    fn files(&mut self, directory: &OwnedHandle, patterns: &[(&str, &str)]) {
        let Some(children) = self.list(directory) else {
            return;
        };
        self.visited = true;
        for child in children {
            if child.directory || !matches_any(patterns, &child.name) {
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
            let (handle, tag) =
                match self.open_child(directory, &child, self.clean && child.directory) {
                    Ok(Some(opened)) => opened,
                    Ok(None) => continue,
                    Err(()) => {
                        empty = false;
                        continue;
                    }
                };
            if child.directory && !is_reparse(&tag) {
                let child_empty = self.walk(&handle);
                if self.stopping() {
                    return false;
                }
                if self.clean && child_empty {
                    // Only empty descendants are removed. A Target folder is never passed here.
                    match delete(&handle) {
                        Ok(()) => {
                            removed = true;
                            self.deleted_any = true;
                        }
                        Err(error) => {
                            empty = false;
                            if win32_code(&error) != Some(ERROR_DIR_NOT_EMPTY) {
                                self.problem(classify(&error), false);
                            }
                        }
                    }
                } else {
                    empty = false;
                }
                continue;
            }
            if self.file(directory, &child, handle, &tag) {
                removed = true;
            } else {
                empty = false;
            }
        }
        empty && removed
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

    /// Scans or cleans one file or link. Returns true when Clean removed it.
    fn file(
        &mut self,
        directory: &OwnedHandle,
        child: &Entry,
        handle: OwnedHandle,
        tag: &FILE_ATTRIBUTE_TAG_INFO,
    ) -> bool {
        let Some(mut bytes) = self.file_bytes(&handle, tag) else {
            return false;
        };
        if !self.clean {
            self.bytes = self.bytes.saturating_add(bytes);
            return false;
        }
        let handle = if child.directory {
            handle
        } else {
            // The inspection handle denies delete sharing, so release it before
            // requesting DELETE. Recheck the reopened file: it may have changed.
            drop(handle);
            if self.stopping() {
                return false;
            }
            let Ok(Some((handle, tag))) = self.open_child(directory, child, true) else {
                return false;
            };
            let Some(current_bytes) = self.file_bytes(&handle, &tag) else {
                return false;
            };
            bytes = current_bytes;
            handle
        };
        if self.stopping() {
            return false;
        }
        match delete(&handle) {
            Ok(()) => {
                self.bytes = self.bytes.saturating_add(bytes);
                self.deleted_any = true;
                true
            }
            Err(error) => {
                self.problem(classify(&error), true);
                false
            }
        }
    }
}

/// True when `name` is `prefix*suffix` for one of `patterns`, ignoring ASCII case.
fn matches_any(patterns: &[(&str, &str)], name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    patterns.iter().any(|(prefix, suffix)| {
        name.len() >= prefix.len() + suffix.len()
            && name
                .get(..prefix.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
            && name
                .get(name.len() - suffix.len()..)
                .is_some_and(|end| end.eq_ignore_ascii_case(suffix))
    })
}

/// Scan one built-in Target at a fixed operation time.
pub fn scan(id: TargetId, roots: &Roots, time: SystemTime, stop: &AtomicBool) -> ScanResult {
    scan_inner(id, roots, time, stop, &mut |_| {})
}

fn scan_inner(
    id: TargetId,
    roots: &Roots,
    time: SystemTime,
    stop: &AtomicBool,
    before_open: &mut dyn FnMut(&OsStr),
) -> ScanResult {
    let Some(target) = targets::find(id) else {
        return ScanResult::Failed {
            problem: Problem::Other,
        };
    };
    let mut walk = Walk::new(false, target, time, stop, before_open);
    walk.target(target, roots);
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

/// Clean one built-in Target. Scan results never authorize a deletion.
pub fn clean(id: TargetId, roots: &Roots, time: SystemTime, stop: &AtomicBool) -> CleanResult {
    clean_inner(id, roots, time, stop, &mut |_| {})
}

fn clean_inner(
    id: TargetId,
    roots: &Roots,
    time: SystemTime,
    stop: &AtomicBool,
    before_open: &mut dyn FnMut(&OsStr),
) -> CleanResult {
    let Some(target) = targets::find(id) else {
        return CleanResult {
            status: CleanStatus::Failed,
            deleted_bytes: 0,
            skipped: vec![],
            coverage_problem: Some(Problem::Other),
        };
    };
    let mut walk = Walk::new(true, target, time, stop, before_open);
    walk.target(target, roots);
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
        deleted_bytes: walk.bytes,
        skipped: walk.skipped,
        coverage_problem: walk.problem,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use filetime::{FileTime, set_file_mtime};
    use std::fs;
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_MODE};

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    /// Roots with one subfolder of `dir` per known folder.
    fn fixture_roots(dir: &Path) -> Roots {
        Roots {
            local_app_data: Ok(dir.join("Local")),
            win_dir: Ok(dir.join("Windows")),
            program_data: Ok(dir.join("ProgramData")),
        }
    }

    fn folder(roots: &Roots, base: Base, path: &str) -> PathBuf {
        roots.base(base).unwrap().join(path)
    }

    /// Fixture roots, the created User temp folder, and a fixed operation time.
    fn fixture() -> (tempfile::TempDir, Roots, PathBuf, SystemTime) {
        let dir = tempfile::tempdir().unwrap();
        let roots = fixture_roots(dir.path());
        let target = folder(&roots, Base::LocalAppData, "Temp");
        fs::create_dir_all(&target).unwrap();
        let time = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        (dir, roots, target, time)
    }

    /// Opens `path`, allowing only `share` access to other opens until the File drops.
    fn hold(path: &Path, share: FILE_SHARE_MODE) -> fs::File {
        fs::OpenOptions::new()
            .read(true)
            .share_mode(share.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(path)
            .unwrap()
    }

    fn write_at(path: &Path, bytes: &[u8], time: SystemTime) {
        fs::write(path, bytes).unwrap();
        set_file_mtime(path, FileTime::from_system_time(time)).unwrap();
    }

    #[test]
    fn failed_known_folder_does_not_block_other_target() {
        for (failed_folder, failed_target, healthy_target) in [
            (FOLDERID_LocalAppData, "user-temp", "windows-temp"),
            (FOLDERID_Windows, "windows-temp", "user-temp"),
        ] {
            let (dir, _, target, time) = fixture();
            write_at(&target.join("old"), b"old", time - DAY - DAY);
            let roots = Roots::resolve(|id| {
                if *id == failed_folder {
                    Err(io::Error::from_raw_os_error(5))
                } else {
                    Ok(dir.path().join("Local"))
                }
            });
            let stop = AtomicBool::new(false);
            assert_eq!(
                scan(failed_target, &roots, time, &stop),
                ScanResult::Failed {
                    problem: Problem::AccessDenied
                }
            );
            let failed_clean = clean(failed_target, &roots, time, &stop);
            assert_eq!(failed_clean.status, CleanStatus::Failed);
            assert_eq!(failed_clean.coverage_problem, Some(Problem::AccessDenied));
            assert_eq!(
                scan(healthy_target, &roots, time, &stop),
                ScanResult::Complete { bytes: 3 }
            );
            let cleaned = clean(healthy_target, &roots, time, &stop);
            assert_eq!(cleaned.status, CleanStatus::Complete);
            assert_eq!(cleaned.deleted_bytes, 3);
        }
    }

    #[test]
    fn redirected_known_folder_does_not_block_other_target() {
        let (dir, _, target, time) = fixture();
        let redirected = dir.path().join("redirected");
        std::os::windows::fs::symlink_dir(target.parent().unwrap(), &redirected).unwrap();
        write_at(&target.join("old"), b"old", time - DAY - DAY);
        let roots = Roots::resolve(|id| {
            Ok(if *id == FOLDERID_LocalAppData {
                redirected.clone()
            } else {
                dir.path().join("Local")
            })
        });
        let stop = AtomicBool::new(false);
        assert_eq!(
            scan("user-temp", &roots, time, &stop),
            ScanResult::Failed {
                problem: Problem::Redirected
            }
        );
        assert_eq!(
            scan("windows-temp", &roots, time, &stop),
            ScanResult::Complete { bytes: 3 }
        );
    }

    #[test]
    fn temp_cutoff_is_strict_and_target_root_survives() {
        let (_dir, roots, target, time) = fixture();
        let old = target.join("old");
        let at = target.join("at");
        let new = target.join("new");
        let future = target.join("future");
        write_at(&old, b"old", time - DAY - Duration::from_secs(1));
        write_at(&at, b"at", time - DAY);
        write_at(&new, b"new", time - DAY + Duration::from_secs(1));
        write_at(&future, b"future", time + DAY);
        let stop = AtomicBool::new(false);
        assert_eq!(
            scan("user-temp", &roots, time, &stop),
            ScanResult::Complete { bytes: 3 }
        );
        let result = clean("user-temp", &roots, time, &stop);
        assert_eq!(result.status, CleanStatus::Complete);
        assert_eq!(result.deleted_bytes, 3);
        assert!(target.is_dir());
        assert!(!old.exists());
        assert!(at.exists() && new.exists() && future.exists());
    }

    #[test]
    fn absent_and_redirected_target_are_not_cleaned() {
        let (dir, roots, target, time) = fixture();
        let stop = AtomicBool::new(false);
        fs::remove_dir(&target).unwrap();
        assert_eq!(
            scan("user-temp", &roots, time, &stop),
            ScanResult::NotPresent
        );
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let sentinel = outside.join("sentinel");
        write_at(&sentinel, b"safe", time - DAY - DAY);
        std::os::windows::fs::symlink_dir(&outside, &target).unwrap();
        assert_eq!(
            scan("user-temp", &roots, time, &stop),
            ScanResult::Failed {
                problem: Problem::Redirected
            }
        );
        assert_eq!(
            clean("user-temp", &roots, time, &stop).status,
            CleanStatus::Failed
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"safe");
    }

    #[test]
    fn directory_replacement_cannot_redirect_scan_or_clean() {
        let (dir, roots, target, time) = fixture();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let sentinel = outside.join("sentinel");
        write_at(&sentinel, b"safe", time - DAY - DAY);
        let child = target.join("move");
        fs::create_dir(&child).unwrap();
        let relocated = dir.path().join("relocated");
        let stop = AtomicBool::new(false);
        let mut swapped = false;
        let scan_result = scan_inner("user-temp", &roots, time, &stop, &mut |name| {
            if name == "move" && !swapped {
                fs::rename(&child, &relocated).unwrap();
                std::os::windows::fs::symlink_dir(&outside, &child).unwrap();
                swapped = true;
            }
        });
        assert!(swapped);
        assert_eq!(scan_result, ScanResult::Complete { bytes: 0 });
        assert_eq!(fs::read(&sentinel).unwrap(), b"safe");

        fs::remove_dir(&child).unwrap();
        fs::create_dir(&child).unwrap();
        let mut swapped = false;
        let cleaned = clean_inner("user-temp", &roots, time, &stop, &mut |name| {
            if name == "move" && !swapped {
                fs::remove_dir(&child).unwrap();
                std::os::windows::fs::symlink_dir(&outside, &child).unwrap();
                swapped = true;
            }
        });
        assert!(swapped);
        assert_eq!(cleaned.deleted_bytes, 0);
        assert_eq!(fs::read(&sentinel).unwrap(), b"safe");
    }

    #[test]
    fn verified_ancestor_cannot_move_during_a_scan() {
        let (dir, roots, target, time) = fixture();
        write_at(&target.join("old"), b"old", time - DAY - DAY);
        let moved = dir.path().join("moved-local");
        let stop = AtomicBool::new(false);
        let mut checked = false;
        let result = scan_inner("user-temp", &roots, time, &stop, &mut |_| {
            if !checked {
                assert!(fs::rename(roots.base(Base::LocalAppData).unwrap(), &moved).is_err());
                checked = true;
            }
        });
        assert!(checked);
        assert_eq!(result, ScanResult::Complete { bytes: 3 });
    }

    #[test]
    fn stop_during_walk_leaves_completed_deletions_and_prevents_new_ones() {
        let (_dir, roots, target, time) = fixture();
        write_at(&target.join("a"), b"a", time - DAY - DAY);
        write_at(&target.join("b"), b"b", time - DAY - DAY);
        let stop = AtomicBool::new(false);
        let mut seen = 0;
        let result = clean_inner("user-temp", &roots, time, &stop, &mut |_| {
            seen += 1;
            if seen == 2 {
                stop.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(result.status, CleanStatus::Stopped);
        assert!(target.join("a").exists() ^ target.join("b").exists());
    }

    #[test]
    fn recent_file_without_delete_sharing_is_excluded_without_failure() {
        let (_dir, roots, target, time) = fixture();
        let file = target.join("working");
        write_at(&file, b"keep", time);
        let locked = hold(&file, FILE_SHARE_READ | FILE_SHARE_WRITE);
        let stop = AtomicBool::new(false);
        assert_eq!(
            scan("user-temp", &roots, time, &stop),
            ScanResult::Complete { bytes: 0 }
        );
        let result = clean("user-temp", &roots, time, &stop);
        assert_eq!(result.status, CleanStatus::Complete, "{result:?}");
        assert_eq!(result.deleted_bytes, 0);
        assert!(result.skipped.is_empty());
        assert_eq!(result.coverage_problem, None);
        assert_eq!(fs::read(&file).unwrap(), b"keep");
        drop(locked);
    }

    #[test]
    fn locked_file_is_reported_and_not_forced() {
        let (_dir, roots, target, time) = fixture();
        let file = target.join("locked");
        write_at(&file, b"data", time - DAY - DAY);
        let locked = hold(&file, FILE_SHARE_MODE(0));
        let stop = AtomicBool::new(false);
        let result = clean("user-temp", &roots, time, &stop);
        assert_eq!(result.status, CleanStatus::Failed);
        assert_eq!(result.skipped, vec![(Problem::SharingViolation, 1)]);
        assert_eq!(result.coverage_problem, None);
        assert!(file.exists());
        drop(locked);
    }

    #[test]
    fn file_opened_with_delete_sharing_can_be_removed() {
        let (_dir, roots, target, time) = fixture();
        let file = target.join("shared");
        write_at(&file, b"data", time - DAY - DAY);
        let shared = hold(
            &file,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        );
        let stop = AtomicBool::new(false);
        let result = clean("user-temp", &roots, time, &stop);
        assert_eq!(result.status, CleanStatus::Complete);
        assert_eq!(result.deleted_bytes, 4);
        drop(shared);
        assert!(!file.exists());
    }

    #[test]
    fn a_file_removed_after_enumeration_contributes_no_bytes() {
        let (_dir, roots, target, time) = fixture();
        let file = target.join("gone");
        write_at(&file, b"data", time - DAY - DAY);
        let stop = AtomicBool::new(false);
        let result = clean_inner("user-temp", &roots, time, &stop, &mut |name| {
            if name == "gone" {
                fs::remove_file(&file).unwrap();
            }
        });
        assert_eq!(result.status, CleanStatus::Complete);
        assert_eq!(result.deleted_bytes, 0);
        assert!(result.skipped.is_empty());
    }

    #[test]
    fn nested_content_is_cleaned_but_recent_files_keep_their_directory() {
        let (_dir, roots, target, time) = fixture();
        let nested = target.join("nested");
        fs::create_dir(&nested).unwrap();
        write_at(&nested.join("old"), b"old", time - DAY - DAY);
        write_at(&nested.join("recent"), b"keep", time - DAY);
        let stop = AtomicBool::new(false);
        let result = clean("user-temp", &roots, time, &stop);
        assert_eq!(result.status, CleanStatus::Complete);
        assert_eq!(result.deleted_bytes, 3);
        assert!(!nested.join("old").exists());
        assert!(nested.join("recent").exists());
    }

    #[test]
    fn empty_descendant_is_removed_after_eligible_content() {
        let (_dir, roots, target, time) = fixture();
        let nested = target.join("nested");
        fs::create_dir(&nested).unwrap();
        write_at(&nested.join("old"), b"old", time - DAY - DAY);
        let stop = AtomicBool::new(false);
        let result = clean("user-temp", &roots, time, &stop);
        assert_eq!(result.status, CleanStatus::Complete);
        assert!(!nested.exists());
        assert!(target.exists());
    }

    #[test]
    fn already_empty_descendant_is_preserved() {
        let (_dir, roots, target, time) = fixture();
        let empty = target.join("fresh-empty");
        fs::create_dir(&empty).unwrap();
        write_at(&target.join("old"), b"old", time - DAY - DAY);
        let stop = AtomicBool::new(false);
        let result = clean("user-temp", &roots, time, &stop);
        assert_eq!(result.status, CleanStatus::Complete);
        assert_eq!(result.deleted_bytes, 3);
        assert!(empty.exists());
        assert!(!target.join("old").exists());
    }

    #[test]
    fn undecodable_filename_does_not_hide_eligible_siblings() {
        let (_dir, roots, target, time) = fixture();
        let unusual = target.join(OsString::from_wide(&[0xd800]));
        write_at(&unusual, b"odd", time - DAY - DAY);
        write_at(&target.join("ordinary"), b"plain", time - DAY - DAY);
        let stop = AtomicBool::new(false);
        assert_eq!(
            scan("user-temp", &roots, time, &stop),
            ScanResult::Complete { bytes: 8 }
        );
        let result = clean("user-temp", &roots, time, &stop);
        assert_eq!(result.status, CleanStatus::Complete);
        assert_eq!(result.deleted_bytes, 8);
        assert!(!unusual.exists());
        assert!(!target.join("ordinary").exists());
    }

    #[test]
    fn windows_temp_uses_only_its_own_root() {
        let (_dir, roots, user_temp, time) = fixture();
        let windows_temp = folder(&roots, Base::WinDir, "Temp");
        fs::create_dir_all(&windows_temp).unwrap();
        write_at(&user_temp.join("user"), b"u", time - DAY - DAY);
        write_at(&windows_temp.join("windows"), b"w", time - DAY - DAY);
        let stop = AtomicBool::new(false);
        assert_eq!(
            scan("windows-temp", &roots, time, &stop),
            ScanResult::Complete { bytes: 1 }
        );
        assert_eq!(clean("windows-temp", &roots, time, &stop).deleted_bytes, 1);
        assert!(user_temp.join("user").exists());
        assert!(!windows_temp.join("windows").exists());
    }

    #[test]
    fn blocked_root_fails_and_blocked_descendant_is_partial() {
        let (_dir, roots, target, time) = fixture();
        let blocked = target.join("blocked");
        fs::create_dir(&blocked).unwrap();
        write_at(&target.join("visible"), b"one", time - DAY - DAY);
        let stop = AtomicBool::new(false);

        let root_lock = hold(&target, FILE_SHARE_MODE(0));
        assert_eq!(
            scan("user-temp", &roots, time, &stop),
            ScanResult::Failed {
                problem: Problem::SharingViolation
            }
        );
        drop(root_lock);

        let child_lock = hold(&blocked, FILE_SHARE_MODE(0));
        assert_eq!(
            scan("user-temp", &roots, time, &stop),
            ScanResult::Partial {
                bytes: 3,
                problem: Problem::SharingViolation
            }
        );
        drop(child_lock);
    }

    #[test]
    fn win32_api_errors_classify_like_nt_errors() {
        let error = os_error(windows::core::Error::from(ERROR_SHARING_VIOLATION));
        assert_eq!(classify(&error), Problem::SharingViolation);
        assert_eq!(
            win32_code(&os_error(ERROR_DIR_NOT_EMPTY.into())),
            Some(ERROR_DIR_NOT_EMPTY)
        );
    }

    #[test]
    fn nonlocal_roots_are_rejected_before_enumeration() {
        let (_dir, mut roots, _target, time) = fixture();
        roots.local_app_data = Ok(PathBuf::from(r"\\server\share\Local"));
        let stop = AtomicBool::new(false);
        assert_eq!(
            scan("user-temp", &roots, time, &stop),
            ScanResult::Failed {
                problem: Problem::NonLocal
            }
        );
    }

    #[test]
    fn inner_symlink_is_removed_without_visiting_its_destination() {
        let (dir, roots, target, time) = fixture();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        write_at(&outside.join("sentinel"), b"safe", time - DAY - DAY);
        let link = target.join("link");
        std::os::windows::fs::symlink_dir(&outside, &link).unwrap();
        let stop = AtomicBool::new(false);
        let result = clean("user-temp", &roots, time, &stop);
        assert_eq!(result.status, CleanStatus::Complete);
        assert_eq!(result.deleted_bytes, 0);
        assert!(!link.exists());
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"safe");
    }

    /// Creates `path` with `bytes`, making its parent folders.
    fn put(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn thumbnail_cache_removes_only_matching_immediate_files() {
        let (_dir, roots, _, time) = fixture();
        let explorer = folder(&roots, Base::LocalAppData, r"Microsoft\Windows\Explorer");
        // No Minimum age applies, so a file written just now is eligible.
        put(&explorer.join("ICONCACHE_48.DB"), b"icon");
        write_at(&explorer.join("thumbcache_256.db"), b"thumb", time);
        let kept = [
            explorer.join("ExplorerStartupLog.etl"),
            explorer.join("thumbcache_256.db.bak"),
            explorer.join(r"nested\thumbcache_32.db"),
        ];
        for path in &kept {
            put(path, b"keep");
        }
        fs::create_dir(explorer.join("iconcache_dir.db")).unwrap();
        let stop = AtomicBool::new(false);
        assert_eq!(
            scan("thumbnail-cache", &roots, time, &stop),
            ScanResult::Complete { bytes: 9 }
        );
        let cleaned = clean("thumbnail-cache", &roots, time, &stop);
        assert_eq!(cleaned.status, CleanStatus::Complete);
        assert_eq!(cleaned.deleted_bytes, 9);
        assert!(!explorer.join("thumbcache_256.db").exists());
        assert!(!explorer.join("ICONCACHE_48.DB").exists());
        assert!(kept.iter().all(|path| path.exists()));
        assert!(explorer.join("iconcache_dir.db").is_dir());
    }

    #[test]
    fn browser_profiles_lose_only_their_fixed_cache_contents() {
        let (_dir, roots, _, time) = fixture();
        let user_data = folder(&roots, Base::LocalAppData, r"Google\Chrome\User Data");
        let stop = AtomicBool::new(false);
        fs::create_dir_all(user_data.join(r"Default\Extensions")).unwrap();
        assert_eq!(
            scan("chrome-cache", &roots, time, &stop),
            ScanResult::NotPresent,
            "a browser without cache folders is absent"
        );

        let user_data_files = [
            "Cookies",
            "History",
            r"Sessions\Session_1",
            "Login Data",
            "Web Data",
            "Preferences",
        ];
        let mut sentinels = vec![user_data.join("Local State")];
        for profile in ["Default", "Profile 2", "Guest Profile", "System Profile"] {
            for file in user_data_files {
                sentinels.push(user_data.join(profile).join(file));
            }
        }
        for profile in ["Guest Profile", "System Profile"] {
            sentinels.push(user_data.join(profile).join(r"Cache\Cache_Data\f_1"));
        }
        for path in &sentinels {
            put(path, b"user data");
        }
        let caches = [
            "Cache\\Cache_Data\\f_1",
            "Code Cache\\js\\index",
            "GPUCache\\data_0",
        ];
        for profile in ["Default", "Profile 2"] {
            for cache in caches {
                put(&user_data.join(profile).join(cache), b"cache");
            }
        }
        assert_eq!(
            scan("chrome-cache", &roots, time, &stop),
            ScanResult::Complete { bytes: 30 }
        );
        let cleaned = clean("chrome-cache", &roots, time, &stop);
        assert_eq!(cleaned.status, CleanStatus::Complete);
        assert_eq!(cleaned.deleted_bytes, 30);
        for profile in ["Default", "Profile 2"] {
            for cache in ["Cache", "Code Cache", "GPUCache"] {
                let cache = user_data.join(profile).join(cache);
                assert!(cache.is_dir());
                assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
            }
        }
        for path in &sentinels {
            assert_eq!(fs::read(path).unwrap(), b"user data", "{}", path.display());
        }
    }

    #[test]
    fn redirected_profile_or_cache_folder_is_skipped_and_reported() {
        let (dir, roots, _, time) = fixture();
        let user_data = folder(&roots, Base::LocalAppData, r"Google\Chrome\User Data");
        let outside = dir.path().join("outside");
        let sentinel = outside.join(r"Cache\sentinel");
        put(&sentinel, b"safe");
        put(&user_data.join(r"Default\Code Cache\js\index"), b"cache");
        fs::create_dir_all(user_data.join("Profile 3")).unwrap();
        std::os::windows::fs::symlink_dir(&outside, user_data.join("Profile 1")).unwrap();
        std::os::windows::fs::symlink_dir(
            outside.join("Cache"),
            user_data.join(r"Profile 3\GPUCache"),
        )
        .unwrap();
        let stop = AtomicBool::new(false);
        assert_eq!(
            scan("chrome-cache", &roots, time, &stop),
            ScanResult::Partial {
                bytes: 5,
                problem: Problem::Redirected
            }
        );
        let cleaned = clean("chrome-cache", &roots, time, &stop);
        assert_eq!(cleaned.status, CleanStatus::Partial);
        assert_eq!(cleaned.deleted_bytes, 5);
        assert_eq!(cleaned.coverage_problem, Some(Problem::Redirected));
        assert_eq!(fs::read(&sentinel).unwrap(), b"safe");
    }

    #[test]
    fn profile_replaced_during_the_walk_cannot_redirect_clean() {
        let (dir, roots, _, time) = fixture();
        let user_data = folder(&roots, Base::LocalAppData, r"Google\Chrome\User Data");
        let profile = user_data.join("Default");
        put(&profile.join(r"Cache\f_1"), b"cache");
        let outside = dir.path().join("outside");
        let sentinel = outside.join(r"Cache\sentinel");
        put(&sentinel, b"safe");
        let stop = AtomicBool::new(false);
        let mut swapped = false;
        let cleaned = clean_inner("chrome-cache", &roots, time, &stop, &mut |name| {
            if name == "Default" && !swapped {
                fs::rename(&profile, dir.path().join("moved")).unwrap();
                std::os::windows::fs::symlink_dir(&outside, &profile).unwrap();
                swapped = true;
            }
        });
        assert!(swapped);
        assert_eq!(cleaned.status, CleanStatus::Failed);
        assert_eq!(cleaned.coverage_problem, Some(Problem::Redirected));
        assert_eq!(fs::read(&sentinel).unwrap(), b"safe");
    }

    #[test]
    fn multi_folder_target_distinguishes_missing_from_inaccessible_folders() {
        let (_dir, roots, _, time) = fixture();
        let stop = AtomicBool::new(false);
        assert_eq!(
            scan("crash-dumps", &roots, time, &stop),
            ScanResult::NotPresent
        );
        let dumps = folder(&roots, Base::LocalAppData, "CrashDumps");
        let queue = folder(
            &roots,
            Base::ProgramData,
            r"Microsoft\Windows\WER\ReportQueue",
        );
        put(&dumps.join("app.exe.123.dmp"), b"dump");
        put(&queue.join(r"Report1\Report.wer"), b"report");
        // The LocalAppData WER folder and ReportArchive are missing, which is normal.
        assert_eq!(
            scan("crash-dumps", &roots, time, &stop),
            ScanResult::Complete { bytes: 10 }
        );

        let blocked = hold(&queue, FILE_SHARE_MODE(0));
        assert_eq!(
            scan("crash-dumps", &roots, time, &stop),
            ScanResult::Partial {
                bytes: 4,
                problem: Problem::SharingViolation
            }
        );
        let cleaned = clean("crash-dumps", &roots, time, &stop);
        assert_eq!(cleaned.status, CleanStatus::Partial);
        assert_eq!(cleaned.deleted_bytes, 4);
        assert_eq!(cleaned.coverage_problem, Some(Problem::SharingViolation));
        drop(blocked);

        fs::remove_dir(&dumps).unwrap();
        let blocked = hold(&queue, FILE_SHARE_MODE(0));
        assert_eq!(
            scan("crash-dumps", &roots, time, &stop),
            ScanResult::Failed {
                problem: Problem::SharingViolation
            }
        );
        drop(blocked);
        assert_eq!(clean("crash-dumps", &roots, time, &stop).deleted_bytes, 6);
        assert!(queue.is_dir());
    }

    #[test]
    fn clean_that_removed_only_empty_files_is_partial_when_a_folder_is_blocked() {
        let (_dir, roots, _, time) = fixture();
        let dumps = folder(&roots, Base::LocalAppData, "CrashDumps");
        let queue = folder(
            &roots,
            Base::ProgramData,
            r"Microsoft\Windows\WER\ReportQueue",
        );
        put(&dumps.join("empty.dmp"), b"");
        fs::create_dir_all(&queue).unwrap();
        let blocked = hold(&queue, FILE_SHARE_MODE(0));
        let stop = AtomicBool::new(false);
        let cleaned = clean("crash-dumps", &roots, time, &stop);
        drop(blocked);
        assert_eq!(cleaned.status, CleanStatus::Partial);
        assert_eq!(cleaned.deleted_bytes, 0);
        assert!(!dumps.join("empty.dmp").exists());
    }

    #[test]
    fn abrupt_exit_leaves_a_usable_partial_target() {
        let (dir, roots, target, time) = fixture();
        write_at(&target.join("a"), b"a", time - DAY - DAY);
        write_at(&target.join("b"), b"b", time - DAY - DAY);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "core::tests::abrupt_exit_helper", "--nocapture"])
            .env("CLEANER_INTERRUPT_DIR", dir.path())
            .status()
            .unwrap();
        assert_eq!(child.code(), Some(17));
        assert!(target.join("a").exists() ^ target.join("b").exists());
        let stop = AtomicBool::new(false);
        assert_eq!(
            clean("user-temp", &roots, time, &stop).status,
            CleanStatus::Complete
        );
        assert!(target.is_dir());
        assert!(!target.join("a").exists() && !target.join("b").exists());
    }

    #[test]
    fn abrupt_exit_helper() {
        let Some(dir) = std::env::var_os("CLEANER_INTERRUPT_DIR") else {
            return;
        };
        let roots = fixture_roots(Path::new(&dir));
        let time = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let stop = AtomicBool::new(false);
        let mut seen = 0;
        let _ = clean_inner("user-temp", &roots, time, &stop, &mut |_| {
            seen += 1;
            if seen == 2 {
                std::process::exit(17);
            }
        });
        panic!("helper did not reach the second file");
    }
}
