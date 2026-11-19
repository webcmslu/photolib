//! Content fingerprint that identifies a photo independently of its name or location.
//!
//! BLAKE3 over: the file size (u64, little-endian), then the first 64 KB, then the
//! last 64 KB. Files of 128 KB or less are hashed whole. Reading at most 128 KB
//! keeps this fast over a network mount even for very large files.
//!
//! Shared organization (albums, favorites) will be keyed by this value, so the
//! definition must never change without a format-version bump.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

/// How much of each end of a file the fingerprint reads.
pub const CHUNK: u64 = 64 * 1024;

/// Returns the fingerprint as 64 lowercase hex characters.
pub fn fingerprint(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    if size <= 2 * CHUNK {
        let mut all = Vec::with_capacity(size as usize);
        file.read_to_end(&mut all)?;
        Ok(from_parts(size, &all, &[]))
    } else {
        let mut head = vec![0u8; CHUNK as usize];
        file.read_exact(&mut head)?;
        let mut tail = vec![0u8; CHUNK as usize];
        file.seek(SeekFrom::End(-(CHUNK as i64)))?;
        file.read_exact(&mut tail)?;
        Ok(from_parts(size, &head, &tail))
    }
}

/// The fingerprint of a file of `size` bytes from what it reads: the whole
/// file in `head` if it is 128 KB or less (and `tail` empty), else its first
/// and last 64 KB. For files that are not on a disk, such as on a phone.
pub fn from_parts(size: u64, head: &[u8], tail: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    hasher.update(head);
    hasher.update(tail);
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn big(fill: u8) -> Vec<u8> {
        vec![fill; 300 * 1024]
    }

    #[test]
    fn stable_across_rename_and_move() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jpg");
        fs::write(&a, big(7)).unwrap();
        let before = fingerprint(&a).unwrap();

        fs::create_dir(dir.path().join("sub")).unwrap();
        let b = dir.path().join("sub/renamed.jpg");
        fs::rename(&a, &b).unwrap();

        assert_eq!(fingerprint(&b).unwrap(), before);
        assert_eq!(before.len(), 64);
    }

    #[test]
    fn differs_when_head_tail_or_size_differ() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, data: &[u8]| {
            let path = dir.path().join(name);
            fs::write(&path, data).unwrap();
            fingerprint(&path).unwrap()
        };

        let base = write("base", &big(1));

        let mut head = big(1);
        head[10] = 2;
        assert_ne!(write("head", &head), base);

        let mut tail = big(1);
        *tail.last_mut().unwrap() = 2;
        assert_ne!(write("tail", &tail), base);

        let mut longer = big(1);
        longer.push(1);
        assert_ne!(write("longer", &longer), base);
    }

    #[test]
    fn small_and_empty_files_work() {
        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("small");
        fs::write(&small, b"hello").unwrap();
        let empty = dir.path().join("empty");
        fs::write(&empty, b"").unwrap();

        assert_ne!(fingerprint(&small).unwrap(), fingerprint(&empty).unwrap());
    }
}
