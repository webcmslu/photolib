//! The fingerprint of a file, as the library computes it
//! (`phototek_core::fingerprint`, not part of the phone app): BLAKE3 over
//! the size, then the first and last 64 KB (or the whole file up to 128 KB).
//! The phone uses it to ask the server which of its photos it already has.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

const CHUNK: u64 = 64 * 1024;

pub fn fingerprint(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    if size <= 2 * CHUNK {
        let mut all = Vec::with_capacity(size as usize);
        file.read_to_end(&mut all)?;
        hasher.update(&all);
    } else {
        let mut chunk = vec![0u8; CHUNK as usize];
        file.read_exact(&mut chunk)?;
        hasher.update(&chunk);
        file.seek(SeekFrom::End(-(CHUNK as i64)))?;
        file.read_exact(&mut chunk)?;
        hasher.update(&chunk);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_same_as_the_librarys() {
        let dir = tempfile::tempdir().unwrap();
        for size in [0usize, 10, 128 * 1024, 128 * 1024 + 1, 1_000_000] {
            let path = dir.path().join(format!("{size}.bin"));
            let bytes: Vec<u8> = (0..size).map(|i| (i * 7 % 251) as u8).collect();
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(
                super::fingerprint(&path).unwrap(),
                phototek_core::fingerprint::fingerprint(&path).unwrap(),
                "{size} bytes"
            );
        }
    }
}
