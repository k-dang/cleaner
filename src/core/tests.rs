use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use filetime::{FileTime, set_file_mtime};
use windows::Win32::Foundation::{ERROR_DIR_NOT_EMPTY, ERROR_SHARING_VIOLATION};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_MODE, FILE_SHARE_READ,
    FILE_SHARE_WRITE,
};
use windows::Win32::UI::Shell::{FOLDERID_LocalAppData, FOLDERID_Windows};

use crate::results::{CleanResult, CleanStatus, Event, Problem, ScanResult};
use crate::targets::{Base, Content, Folders, TargetId};

use super::recycle_bin::tests::{FakeShell, bin, drive};
use super::win::{classify, os_error, win32_code};
use super::*;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The folders and Minimum age of a built-in folder Target.
fn folders(id: TargetId) -> (&'static Folders, Option<Duration>) {
    match &target(id).content {
        Content::Folders { folders, min_age } => (folders, *min_age),
        Content::RecycleBin => panic!("{id} is not a folder Target"),
    }
}

fn scan_inner(
    id: TargetId,
    roots: &Roots,
    time: SystemTime,
    stop: &AtomicBool,
    before_open: &mut dyn FnMut(&OsStr),
) -> ScanResult {
    let (folders, min_age) = folders(id);
    scan_folders(folders, min_age, roots, time, stop, before_open)
}

fn clean_inner(
    id: TargetId,
    roots: &Roots,
    time: SystemTime,
    stop: &AtomicBool,
    before_open: &mut dyn FnMut(&OsStr),
) -> CleanResult {
    let (folders, min_age) = folders(id);
    clean_folders(folders, min_age, roots, time, stop, before_open)
}

fn scan(id: TargetId, roots: &Roots, time: SystemTime, stop: &AtomicBool) -> ScanResult {
    scan_inner(id, roots, time, stop, &mut |_| {})
}

fn clean(id: TargetId, roots: &Roots, time: SystemTime, stop: &AtomicBool) -> CleanResult {
    clean_inner(id, roots, time, stop, &mut |_| {})
}

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
        assert_eq!(cleaned.deleted_bytes, Some(3));
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
    assert_eq!(result.deleted_bytes, Some(3));
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
    assert_eq!(cleaned.deleted_bytes, Some(0));
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
    let mut first = None;
    let result = clean_inner("user-temp", &roots, time, &stop, &mut |name| {
        if *first.get_or_insert_with(|| name.to_owned()) != name {
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
    assert_eq!(result.deleted_bytes, Some(0));
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
    assert_eq!(result.deleted_bytes, Some(4));
    // The name stays listed, pending deletion, until the holder closes. It is
    // already gone, not inaccessible, so it must not block the next Clean.
    assert_eq!(
        scan("user-temp", &roots, time, &stop),
        ScanResult::Complete { bytes: 0 }
    );
    let again = clean("user-temp", &roots, time, &stop);
    assert_eq!(again.status, CleanStatus::Complete, "{again:?}");
    assert_eq!(again.deleted_bytes, Some(0));
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
    assert_eq!(result.deleted_bytes, Some(0));
    assert!(result.skipped.is_empty());
}

#[test]
fn directory_without_delete_sharing_still_has_its_contents_cleaned() {
    let (_dir, roots, target, time) = fixture();
    let nested = target.join("nested");
    fs::create_dir(&nested).unwrap();
    let file = nested.join("old");
    write_at(&file, b"data", time - DAY - DAY);
    let locked = hold(&nested, FILE_SHARE_READ | FILE_SHARE_WRITE);
    let stop = AtomicBool::new(false);
    assert_eq!(
        scan("user-temp", &roots, time, &stop),
        ScanResult::Complete { bytes: 4 }
    );
    let result = clean("user-temp", &roots, time, &stop);
    assert_eq!(result.deleted_bytes, Some(4), "{result:?}");
    assert_eq!(result.status, CleanStatus::Partial);
    assert!(result.skipped.is_empty());
    assert_eq!(result.coverage_problem, Some(Problem::SharingViolation));
    assert!(!file.exists());
    assert!(nested.is_dir());
    assert!(target.is_dir());
    drop(locked);
}

#[test]
fn nested_content_is_cleaned_but_recent_files_keep_their_directory() {
    let (_dir, roots, target, time) = fixture();
    let nested = target.join("nested");
    fs::create_dir(&nested).unwrap();
    write_at(&nested.join("old"), b"old", time - DAY - DAY);
    write_at(&nested.join("recent"), b"keep", time - DAY);
    let locked = hold(&nested, FILE_SHARE_READ | FILE_SHARE_WRITE);
    let stop = AtomicBool::new(false);
    let result = clean("user-temp", &roots, time, &stop);
    assert_eq!(result.status, CleanStatus::Complete);
    assert_eq!(result.deleted_bytes, Some(3));
    assert!(!nested.join("old").exists());
    assert!(nested.join("recent").exists());
    drop(locked);
}

#[test]
fn directory_replaced_before_pruning_is_preserved() {
    let (dir, roots, target, time) = fixture();
    let nested = target.join("nested");
    fs::create_dir(&nested).unwrap();
    write_at(&nested.join("old"), b"data", time - DAY - DAY);
    let moved = dir.path().join("moved");
    let stop = AtomicBool::new(false);
    let mut opens = 0;
    let result = clean_inner("user-temp", &roots, time, &stop, &mut |name| {
        if name == "nested" {
            opens += 1;
            if opens == 2 {
                fs::rename(&nested, &moved).unwrap();
                fs::create_dir(&nested).unwrap();
            }
        }
    });
    assert_eq!(opens, 2);
    assert_eq!(result.status, CleanStatus::Partial);
    assert_eq!(result.deleted_bytes, Some(4));
    assert_eq!(result.coverage_problem, Some(Problem::Redirected));
    assert!(result.skipped.is_empty());
    assert!(nested.is_dir());
    assert!(moved.is_dir());
    assert!(!moved.join("old").exists());
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
    assert_eq!(result.deleted_bytes, Some(3));
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
    assert_eq!(result.deleted_bytes, Some(8));
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
    assert_eq!(
        clean("windows-temp", &roots, time, &stop).deleted_bytes,
        Some(1)
    );
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
    assert_eq!(result.deleted_bytes, Some(0));
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
    put(&explorer.join("THUMBCACHE_48.DB"), b"thumb");
    write_at(&explorer.join("thumbcache_256.db"), b"thumb", time);
    let kept = [
        explorer.join("iconcache_48.db"),
        explorer.join("ExplorerStartupLog.etl"),
        explorer.join("thumbcache_256.db.bak"),
        explorer.join(r"nested\thumbcache_32.db"),
    ];
    for path in &kept {
        put(path, b"keep");
    }
    fs::create_dir(explorer.join("thumbcache_dir.db")).unwrap();
    let stop = AtomicBool::new(false);
    assert_eq!(
        scan("thumbnail-cache", &roots, time, &stop),
        ScanResult::Complete { bytes: 10 }
    );
    let cleaned = clean("thumbnail-cache", &roots, time, &stop);
    assert_eq!(cleaned.status, CleanStatus::Complete);
    assert_eq!(cleaned.deleted_bytes, Some(10));
    assert!(!explorer.join("thumbcache_256.db").exists());
    assert!(!explorer.join("THUMBCACHE_48.DB").exists());
    assert!(kept.iter().all(|path| path.exists()));
    assert!(explorer.join("thumbcache_dir.db").is_dir());
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
    assert_eq!(cleaned.deleted_bytes, Some(30));
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
    std::os::windows::fs::symlink_dir(outside.join("Cache"), user_data.join(r"Profile 3\GPUCache"))
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
    assert_eq!(cleaned.deleted_bytes, Some(5));
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
    assert_eq!(cleaned.deleted_bytes, Some(4));
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
    assert_eq!(
        clean("crash-dumps", &roots, time, &stop).deleted_bytes,
        Some(6)
    );
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
    assert_eq!(cleaned.deleted_bytes, Some(0));
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
fn mixed_scan_and_clean_report_each_target_in_order() {
    let (_dir, roots, target, time) = fixture();
    write_at(&target.join("old"), b"old", time - DAY - DAY);
    let shell = FakeShell::new(&[('C', bin(300, 2)), ('D', bin(0, 0))]);
    let stop = AtomicBool::new(false);
    let ids = ["recycle-bin", "user-temp"];
    let mut events = Vec::new();
    scan_targets(&ids, &roots, &shell, time, &stop, &mut |e| events.push(e));
    assert_eq!(
        events,
        [
            Event::Scanning("recycle-bin"),
            Event::RecycleBinScanned(
                "recycle-bin",
                ScanResult::Complete { bytes: 300 },
                vec![drive('C'), drive('D')]
            ),
            Event::Scanning("user-temp"),
            Event::Scanned("user-temp", ScanResult::Complete { bytes: 3 }),
            Event::Finished,
        ]
    );
    events.clear();
    clean_targets(&ids, &[drive('C')], &roots, &shell, time, &stop, &mut |e| {
        events.push(e)
    });
    let emptied = CleanResult {
        status: CleanStatus::Complete,
        deleted_bytes: None,
        skipped: vec![],
        coverage_problem: None,
    };
    assert_eq!(
        events,
        [
            Event::Cleaning("recycle-bin"),
            Event::Cleaned("recycle-bin", emptied.clone()),
            Event::Cleaning("user-temp"),
            Event::Cleaned(
                "user-temp",
                CleanResult {
                    deleted_bytes: Some(3),
                    ..emptied
                }
            ),
            Event::Finished,
        ]
    );
    assert!(!target.join("old").exists());
}

#[test]
fn abrupt_exit_helper() {
    let Some(dir) = std::env::var_os("CLEANER_INTERRUPT_DIR") else {
        return;
    };
    let roots = fixture_roots(Path::new(&dir));
    let time = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
    let stop = AtomicBool::new(false);
    let mut first = None;
    let _ = clean_inner("user-temp", &roots, time, &stop, &mut |name| {
        if *first.get_or_insert_with(|| name.to_owned()) != name {
            std::process::exit(17);
        }
    });
    panic!("helper did not reach the second file");
}
