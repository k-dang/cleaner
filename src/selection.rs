//! Durable Selection storage. The file contains only built-in Target IDs and booleans.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows::core::PCWSTR;

use crate::targets::TARGETS;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

pub type Choices = BTreeMap<String, bool>;

pub struct SelectionStore {
    path: PathBuf,
}

pub struct Loaded {
    pub choices: Choices,
    pub needs_save: bool,
}

impl SelectionStore {
    pub fn system() -> io::Result<Self> {
        Self::resolve(crate::core::validated_known_folder)
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
        let contents = match fs::read(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Loaded {
                    choices: defaults(),
                    needs_save: true,
                });
            }
            Err(error) => return Err(error),
        };
        let value: serde_json::Value = serde_json::from_slice(&contents)?;
        let object = value.as_object().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Selection must be a JSON object",
            )
        })?;
        if object.values().any(|value| !value.is_boolean()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Selection values must be booleans",
            ));
        }
        let mut choices = defaults();
        let mut needs_save = object.len() != choices.len();
        for (id, selected) in &mut choices {
            match object.get(id) {
                Some(value) => *selected = value.as_bool().unwrap(),
                None => needs_save = true,
            }
        }
        Ok(Loaded {
            choices,
            needs_save,
        })
    }

    /// A synced temporary file is renamed over the old file on the same volume.
    pub fn save(&self, choices: &Choices) -> io::Result<()> {
        let expected = defaults();
        if choices.len() != expected.len() || expected.keys().any(|id| !choices.contains_key(id)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "incomplete Selection",
            ));
        }
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
            serde_json::to_writer(&mut file, choices)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);
            let from = wide(&temp);
            let to = wide(&self.path);
            // SAFETY: Both paths are null-terminated and live for the call.
            unsafe {
                MoveFileExW(
                    PCWSTR(from.as_ptr()),
                    PCWSTR(to.as_ptr()),
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

pub fn defaults() -> Choices {
    TARGETS
        .iter()
        .map(|target| (target.id.to_string(), target.default_selected))
        .collect()
}

pub fn none_selected() -> Choices {
    TARGETS
        .iter()
        .map(|target| (target.id.to_string(), false))
        .collect()
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn reject_redirected(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_attributes() & 0x400 != 0 => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Selection directory is redirected",
        )),
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
    fn missing_new_ids_take_defaults_and_obsolete_ids_are_ignored() {
        let (_dir, store) = store();
        fs::write(&store.path, br#"{"user-temp":false,"obsolete":true}"#).unwrap();
        let loaded = store.load().unwrap();
        assert!(loaded.needs_save);
        assert!(!loaded.choices["user-temp"]);
        assert!(loaded.choices["windows-temp"]);
        assert!(!loaded.choices.contains_key("obsolete"));
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
        let mut choices = defaults();
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
        let mut choices = defaults();
        for selected in [false, true, false, true, false] {
            choices.insert("user-temp".into(), selected);
            store.save(&choices).unwrap();
        }
        assert!(!store.load().unwrap().choices["user-temp"]);
    }

    #[test]
    fn failed_replacement_keeps_the_previous_complete_selection() {
        let (_dir, store) = store();
        let original = defaults();
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
    fn redirected_selection_directory_is_not_read_or_written() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let link = dir.path().join("selection-dir");
        std::os::windows::fs::symlink_dir(&outside, &link).unwrap();
        let store = SelectionStore::new(link.join("choices.json"));
        assert!(store.load().is_err());
        assert!(store.save(&defaults()).is_err());
        assert!(!outside.join("choices.json").exists());
    }
}
