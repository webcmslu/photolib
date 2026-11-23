//! Photolib became Phototek, and the app's identifier went from
//! `lu.webcms.photolib` to `lu.webcms.phototek`. Each system keeps an app's
//! data in folders named after that identifier: the index, thumbnails,
//! face models, this computer's `device-id` (which the library's shared
//! logs know it by) and the window's own storage (language, grid size,
//! last folders). At the first start under the new name, those folders
//! are moved over, before the window opens, so nothing is lost.
//!
//! What a library shares stays where it is: the `.photolib` folder inside
//! the library is not renamed, so computers that still run an older
//! version keep working on the same library.

use std::path::{Path, PathBuf};

/// The identifier the app had as Photolib.
const OLD_IDENTIFIER: &str = "lu.webcms.photolib";
/// The identifier in tauri.conf.json (a test keeps the two the same).
const IDENTIFIER: &str = "lu.webcms.phototek";

/// Moves the data the app kept as Photolib, where there is some.
pub fn move_old_data() {
    for parent in parents() {
        if let Err(error) = move_folder(&parent, OLD_IDENTIFIER, IDENTIFIER) {
            eprintln!(
                "Phototek: couldn't move {OLD_IDENTIFIER} in {}: {error}",
                parent.display()
            );
        }
    }
}

/// The folders that hold one folder per app, named after its identifier:
/// where Tauri puts the app's data, local data, cache and config, and on
/// macOS where WebKit keeps the window's storage.
fn parents() -> Vec<PathBuf> {
    #[allow(unused_mut)]
    let mut parents: Vec<PathBuf> = [
        dirs::data_dir(),
        dirs::data_local_dir(),
        dirs::cache_dir(),
        dirs::config_dir(),
    ]
    .into_iter()
    .flatten()
    .collect();
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        parents.push(home.join("Library/WebKit"));
        parents.push(home.join("Library/HTTPStorages"));
    }
    parents.sort();
    parents.dedup();
    parents
}

/// `<parent>/<old>` becomes `<parent>/<new>`. Nothing happens when there is
/// no old folder, or when the new one exists already: then it's the one in
/// use, and it isn't overwritten. Returns whether something moved.
fn move_folder(parent: &Path, old: &str, new: &str) -> std::io::Result<bool> {
    let from = parent.join(old);
    let to = parent.join(new);
    if !from.is_dir() || to.exists() {
        return Ok(false);
    }
    std::fs::rename(&from, &to)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("phototek-rename-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_identifier_is_the_one_in_the_config() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], IDENTIFIER);
        assert_ne!(OLD_IDENTIFIER, IDENTIFIER);
    }

    #[test]
    fn old_data_moves_to_the_new_name() {
        let parent = scratch("moves");
        fs::create_dir_all(parent.join("old.id/models")).unwrap();
        fs::write(parent.join("old.id/device-id"), "mac-1").unwrap();

        assert!(move_folder(&parent, "old.id", "new.id").unwrap());
        assert_eq!(
            fs::read_to_string(parent.join("new.id/device-id")).unwrap(),
            "mac-1"
        );
        assert!(parent.join("new.id/models").is_dir());
        assert!(!parent.join("old.id").exists());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn data_under_the_new_name_is_never_overwritten() {
        let parent = scratch("keeps");
        fs::create_dir_all(parent.join("old.id")).unwrap();
        fs::write(parent.join("old.id/device-id"), "old").unwrap();
        fs::create_dir_all(parent.join("new.id")).unwrap();
        fs::write(parent.join("new.id/device-id"), "new").unwrap();

        assert!(!move_folder(&parent, "old.id", "new.id").unwrap());
        assert_eq!(
            fs::read_to_string(parent.join("new.id/device-id")).unwrap(),
            "new"
        );
        assert!(parent.join("old.id").exists());

        // Nothing to move: nothing happens.
        assert!(!move_folder(&parent, "gone.id", "other.id").unwrap());
        assert!(!parent.join("other.id").exists());
        fs::remove_dir_all(parent).unwrap();
    }
}
