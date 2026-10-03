//! Win32 handles, errors, and paths shared by the folder walk and the Recycle Bin.

use std::ffi::OsStr;
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf, Prefix};

use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT, NtCreateFile,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, HANDLE,
    OBJ_CASE_INSENSITIVE, RPC_E_CHANGED_MODE, RtlNtStatusToDosError, STATUS_DELETE_PENDING,
    UNICODE_STRING, WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
    FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAGS_AND_ATTRIBUTES, FILE_INFO_BY_HANDLE_CLASS,
    FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FileAttributeTagInfo, FileDispositionInfoEx, GetDriveTypeW, GetFileInformationByHandleEx,
    OPEN_EXISTING, SYNCHRONIZE, SetFileInformationByHandle,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::IO::IO_STATUS_BLOCK;
use windows::Win32::System::SystemServices::{IO_REPARSE_TAG_MOUNT_POINT, IO_REPARSE_TAG_SYMLINK};
use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;
use windows::Win32::UI::Shell::{KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath};
use windows::core::{GUID, HSTRING, PWSTR};

use crate::results::Problem;

pub(super) struct OwnedHandle(pub(super) HANDLE);

pub(super) struct RootHandles(Vec<OwnedHandle>);

impl RootHandles {
    pub(super) fn target(&self) -> &OwnedHandle {
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

pub(super) fn known_folder(id: &GUID) -> io::Result<PathBuf> {
    with_com(|| known_folder_initialized(id))
}

/// Runs `f` with COM initialized on this thread, as shell calls require. GPUI
/// may already have initialized it with a different apartment model.
pub(super) fn with_com<T>(f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    // SAFETY: no reserved pointer is supplied and this function balances a
    // successful initialization before returning.
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if initialized.is_err() && initialized != RPC_E_CHANGED_MODE {
        return Err(os_error(windows::core::Error::from(initialized)));
    }
    let owns_com = initialized.is_ok();
    let result = f();
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
pub(super) fn os_error(error: windows::core::Error) -> io::Error {
    match WIN32_ERROR::from_error(&error) {
        Some(code) => io::Error::from_raw_os_error(code.0.cast_signed()),
        None => io::Error::from(error),
    }
}

pub(super) fn win32_code(error: &io::Error) -> Option<WIN32_ERROR> {
    error
        .raw_os_error()
        .map(|code| WIN32_ERROR(code.cast_unsigned()))
}

pub(super) fn classify(error: &io::Error) -> Problem {
    match win32_code(error) {
        Some(ERROR_ACCESS_DENIED) => Problem::AccessDenied,
        Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION) => Problem::SharingViolation,
        _ => Problem::Other,
    }
}

pub(super) fn relative_open(
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
    // A name whose deletion is pending until other handles close is already being
    // removed, so it is treated as gone rather than as access denied, which is
    // what the Win32 conversion below would report.
    if code == STATUS_DELETE_PENDING {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "deletion is pending",
        ));
    }
    if code.is_err() {
        // SAFETY: RtlNtStatusToDosError accepts any NTSTATUS value.
        let win32 = unsafe { RtlNtStatusToDosError(code) };
        return Err(io::Error::from_raw_os_error(win32.cast_signed()));
    }
    Ok(OwnedHandle(handle))
}

pub(super) fn info<T: Default>(
    handle: &OwnedHandle,
    class: FILE_INFO_BY_HANDLE_CLASS,
) -> io::Result<T> {
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

pub(super) fn is_reparse(tag: &FILE_ATTRIBUTE_TAG_INFO) -> bool {
    tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

/// A symlink or mount point: removable as a name without visiting its destination.
pub(super) fn is_link(tag: &FILE_ATTRIBUTE_TAG_INFO) -> bool {
    is_reparse(tag)
        && (tag.ReparseTag == IO_REPARSE_TAG_SYMLINK
            || tag.ReparseTag == IO_REPARSE_TAG_MOUNT_POINT)
}

/// The root of a drive, such as `C:\`.
pub(super) fn drive_root(letter: char) -> HSTRING {
    HSTRING::from(format!("{letter}:\\"))
}

/// True when `root` is the root of a local fixed drive.
pub(super) fn is_fixed_drive(root: &HSTRING) -> bool {
    // SAFETY: `root` is terminated and lives through the call.
    unsafe { GetDriveTypeW(root) == DRIVE_FIXED }
}

pub(super) fn root_handle(path: &Path) -> io::Result<Option<RootHandles>> {
    let mut components = path.components();
    let drive = match (components.next(), components.next()) {
        (Some(Component::Prefix(prefix)), Some(Component::RootDir)) => match prefix.kind() {
            Prefix::Disk(letter) => Some(drive_root(char::from(letter))),
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
    if !is_fixed_drive(&drive) {
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
pub(super) fn open_dir(parent: &OwnedHandle, name: &OsStr) -> io::Result<Option<OwnedHandle>> {
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

pub(super) fn root_problem(error: &io::Error) -> Problem {
    match error.kind() {
        io::ErrorKind::InvalidData => Problem::Redirected,
        io::ErrorKind::InvalidInput => Problem::NonLocal,
        _ => classify(error),
    }
}

pub(super) fn delete(handle: &OwnedHandle) -> io::Result<()> {
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
