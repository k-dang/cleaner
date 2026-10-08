//! Durable Selection storage. The file contains only built-in Target IDs and booleans.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows::core::HSTRING;

use crate::targets;
use cleaner_core::selection::{self, Choices, Loaded};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub struct SelectionStore {
    path: PathBuf,
}

impl SelectionStore {
    pub fn system() -> io::Result<Self> {
        Self::resolve(crate::cleanup::validated_known_folder)
    }

    fn resolve(
        folder: impl FnOnce(&windows::core::GUID) -> io::Result<PathBuf>,
    ) -> io::Result<Self> {
        let app_data = folder(&windows::Win32::UI::Shell::FOLDERID_RoamingAppData)?;
        Ok(Self::new(
            app_data.join("cc-cleaner-at-home").join("selection.json"),
        ))
    }

    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> io::Result<Loaded> {
        if let Some(parent) = self.path.parent() {
            reject_redirected(parent)?;
        }
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            // Delete sharing lets a save replace the file while it is read.
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Loaded {
                    choices: selection::defaults(targets::catalog()),
                    needs_save: true,
                });
            }
            Err(error) => return Err(error),
        };
        // Inspect the same handle we read, so a replaced or dangling link cannot
        // redirect the read or be mistaken for an absent Selection.
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Selection file is redirected",
            ));
        }
        let mut contents = Vec::new();
        file.read_to_end(&mut contents)?;
        selection::parse(&contents, targets::catalog())
    }

    /// A synced temporary file is renamed over the old file on the same volume.
    pub fn save(&self, choices: &Choices) -> io::Result<()> {
        let contents = selection::serialize(choices, targets::catalog())?;
        let parent = self.path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Selection path has no parent")
        })?;
        reject_redirected(parent)?;
        fs::create_dir_all(parent)?;
        reject_redirected(parent)?;
        let temp = self.path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        let outcome = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(&contents)?;
            file.sync_all()?;
            drop(file);
            // SAFETY: Both paths are null-terminated and live for the call.
            unsafe {
                MoveFileExW(
                    &HSTRING::from(temp.as_os_str()),
                    &HSTRING::from(self.path.as_os_str()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
                .map_err(io::Error::from)
            }
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(&temp);
        }
        outcome
    }
}

fn reject_redirected(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 => {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Selection directory is redirected",
            ))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SelectionStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SelectionStore::new(dir.path().join("choices.json"));
        (dir, store)
    }

    #[test]
    fn storage_only_resolves_roaming_app_data() {
        let dir = tempfile::tempdir().unwrap();
        let store = SelectionStore::resolve(|id| {
            if *id == windows::Win32::UI::Shell::FOLDERID_RoamingAppData {
                Ok(dir.path().to_path_buf())
            } else {
                Err(io::Error::from_raw_os_error(5))
            }
        })
        .unwrap();
        let mut choices = store.load().unwrap().choices;
        choices.insert("user-temp".into(), false);
        store.save(&choices).unwrap();
        assert_eq!(store.load().unwrap().choices, choices);
    }

    #[test]
    fn first_run_and_explicit_untick_survive_restart() {
        let (_dir, store) = store();
        let mut loaded = store.load().unwrap();
        assert!(loaded.needs_save);
        store.save(&loaded.choices).unwrap();
        loaded.choices.insert("user-temp".into(), false);
        store.save(&loaded.choices).unwrap();
        let restarted = store.load().unwrap();
        assert!(!restarted.needs_save);
        assert!(!restarted.choices["user-temp"]);
        assert!(restarted.choices["windows-temp"]);
    }

    #[test]
    fn malformed_file_is_an_error_and_leftover_temps_are_ignored() {
        let (_dir, store) = store();
        fs::write(&store.path, b"{").unwrap();
        fs::write(store.path.with_extension("old.tmp"), b"{}").unwrap();
        assert!(store.load().is_err());
    }

    #[test]
    fn interrupted_temp_write_preserves_the_last_complete_selection() {
        let (_dir, store) = store();
        let mut choices = selection::defaults(targets::catalog());
        choices.insert("user-temp".into(), false);
        store.save(&choices).unwrap();
        fs::write(
            store.path.with_extension("interrupted.tmp"),
            b"{\"user-temp\":",
        )
        .unwrap();
        assert_eq!(store.load().unwrap().choices, choices);
    }

    #[test]
    fn ordered_changes_save_the_last_explicit_choice() {
        let (_dir, store) = store();
        let mut choices = selection::defaults(targets::catalog());
        for selected in [false, true, false, true, false] {
            choices.insert("user-temp".into(), selected);
            store.save(&choices).unwrap();
        }
        assert!(!store.load().unwrap().choices["user-temp"]);
    }

    #[test]
    fn failed_replacement_keeps_the_previous_complete_selection() {
        let (_dir, store) = store();
        let original = selection::defaults(targets::catalog());
        store.save(&original).unwrap();
        let original_permissions = fs::metadata(&store.path).unwrap().permissions();
        let mut readonly = original_permissions.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&store.path, readonly).unwrap();
        let mut changed = original.clone();
        changed.insert("user-temp".into(), false);
        assert!(store.save(&changed).is_err());
        assert_eq!(store.load().unwrap().choices, original);
        fs::set_permissions(&store.path, original_permissions).unwrap();
    }

    #[test]
    fn load_shares_delete_access_with_other_handles() {
        let (_dir, store) = store();
        let mut choices = selection::defaults(targets::catalog());
        choices.insert("user-temp".into(), false);
        store.save(&choices).unwrap();
        // Another handle that may delete or replace the file, such as a pending save.
        let _other = OpenOptions::new()
            .access_mode(windows::Win32::Storage::FileSystem::DELETE.0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0)
            .open(&store.path)
            .unwrap();
        assert_eq!(store.load().unwrap().choices, choices);
    }

    #[test]
    fn redirected_selection_file_is_a_load_error_even_when_dangling() {
        let (dir, store) = store();
        let destination = dir.path().join("saved.json");
        fs::write(&destination, br#"{"user-temp":false}"#).unwrap();
        std::os::windows::fs::symlink_file(&destination, &store.path).unwrap();
        let existing = store.load();
        fs::remove_file(&destination).unwrap();
        let dangling = store.load();
        assert!(existing.is_err(), "redirected Selection was read");
        assert!(dangling.is_err(), "dangling Selection restored defaults");
    }

    #[test]
    fn redirected_selection_directory_is_not_read_or_written() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let link = dir.path().join("selection-dir");
        std::os::windows::fs::symlink_dir(&outside, &link).unwrap();
        let store = SelectionStore::new(link.join("choices.json"));
        assert!(store.load().is_err());
        assert!(
            store
                .save(&selection::defaults(targets::catalog()))
                .is_err()
        );
        assert!(!outside.join("choices.json").exists());
    }
}
