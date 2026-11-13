//! The "recent libraries" list, stored as a small JSON file in the app-data dir.
//!
//! This is per-machine state, so it lives locally and never in the library folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// How many libraries the list remembers.
pub const MAX_RECENTS: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentEntry {
    pub path: String,
    pub name: String,
    /// Unix time in milliseconds.
    pub last_opened: u64,
}

/// Moves `path` to the front of the list (adding it if needed) and trims the list.
pub fn touch(list: &mut Vec<RecentEntry>, path: &str, name: &str, now_ms: u64) {
    list.retain(|entry| entry.path != path);
    list.insert(
        0,
        RecentEntry {
            path: path.to_owned(),
            name: name.to_owned(),
            last_opened: now_ms,
        },
    );
    list.truncate(MAX_RECENTS);
}

/// Removes `path` from the list. Does nothing if it isn't there.
pub fn remove(list: &mut Vec<RecentEntry>, path: &str) {
    list.retain(|entry| entry.path != path);
}

/// Reads and writes the list on disk.
pub struct RecentStore {
    file: PathBuf,
}

impl RecentStore {
    pub fn new(app_data_dir: &Path) -> Self {
        Self {
            file: app_data_dir.join("recent-libraries.json"),
        }
    }

    /// Loads the list. A missing or unreadable file gives an empty list: losing
    /// the recents is harmless, and refusing to start over it would not be.
    pub fn load(&self) -> Vec<RecentEntry> {
        fs::read(&self.file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Saves the list by writing a temp file and renaming it over the old one,
    /// so a crash mid-write can't leave a half-written file.
    pub fn save(&self, list: &[RecentEntry]) -> io::Result<()> {
        if let Some(dir) = self.file.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = self.file.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(list)?)?;
        fs::rename(&tmp, &self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[RecentEntry]) -> Vec<&str> {
        list.iter().map(|entry| entry.path.as_str()).collect()
    }

    #[test]
    fn touch_puts_newest_first() {
        let mut list = Vec::new();
        touch(&mut list, "/a", "a", 1);
        touch(&mut list, "/b", "b", 2);
        assert_eq!(paths(&list), ["/b", "/a"]);
    }

    #[test]
    fn touch_moves_existing_entry_to_front_without_duplicating() {
        let mut list = Vec::new();
        touch(&mut list, "/a", "a", 1);
        touch(&mut list, "/b", "b", 2);
        touch(&mut list, "/a", "a", 3);
        assert_eq!(paths(&list), ["/a", "/b"]);
        assert_eq!(list[0].last_opened, 3);
    }

    #[test]
    fn touch_caps_the_list() {
        let mut list = Vec::new();
        for i in 0..(MAX_RECENTS as u64 + 5) {
            touch(&mut list, &format!("/{i}"), "x", i);
        }
        assert_eq!(list.len(), MAX_RECENTS);
        assert_eq!(list[0].path, format!("/{}", MAX_RECENTS + 4));
    }

    #[test]
    fn remove_drops_only_the_given_path() {
        let mut list = Vec::new();
        touch(&mut list, "/a", "a", 1);
        touch(&mut list, "/b", "b", 2);
        remove(&mut list, "/a");
        remove(&mut list, "/missing");
        assert_eq!(paths(&list), ["/b"]);
    }

    #[test]
    fn store_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = RecentStore::new(&dir.path().join("nested"));
        assert!(store.load().is_empty());

        let mut list = Vec::new();
        touch(&mut list, "/photos/été", "été", 42);
        store.save(&list).unwrap();
        assert_eq!(store.load(), list);
    }

    #[test]
    fn corrupt_file_loads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("recent-libraries.json"), b"{not json").unwrap();
        assert!(RecentStore::new(dir.path()).load().is_empty());
    }
}
