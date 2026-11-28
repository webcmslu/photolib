//! The neural networks the library runs (faces, search by content): not
//! part of the installer, they are downloaded the first time they are
//! needed, into the app's data folder, and checked before use.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub(crate) struct ModelFile {
    pub name: &'static str,
    /// Where it can be downloaded, tried in this order: the original, then
    /// our own copies, in case the original goes away. Any copy is checked
    /// against `hash`, so a mirror can only serve the very same file.
    pub urls: &'static [&'static str],
    /// BLAKE3 hash of the file.
    pub hash: &'static str,
    pub size: u64,
}

fn dir(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("models")
}

/// Where `model` is (or will be) on this computer.
pub(crate) fn path(app_data_dir: &Path, model: &ModelFile) -> PathBuf {
    dir(app_data_dir).join(model.name)
}

/// True once all of `models` are on this computer.
pub(crate) fn present(app_data_dir: &Path, models: &[&ModelFile]) -> bool {
    models.iter().all(|model| {
        fs::metadata(path(app_data_dir, model)).is_ok_and(|meta| meta.len() == model.size)
    })
}

/// Downloads those of `models` that are missing. `on_progress` gets the
/// bytes received so far, out of their total size.
pub(crate) fn download(
    app_data_dir: &Path,
    models: &[&ModelFile],
    on_progress: &dyn Fn(u64),
) -> io::Result<()> {
    fs::create_dir_all(dir(app_data_dir))?;
    let mut done = 0u64;
    for model in models {
        let target = path(app_data_dir, model);
        if fs::metadata(&target).is_ok_and(|meta| meta.len() == model.size) {
            done += model.size;
            on_progress(done);
            continue;
        }
        let mut failures = Vec::new();
        let mut found = None;
        for url in model.urls {
            match fetch(model, url, &|got| on_progress(done + got)) {
                Ok(bytes) => {
                    found = Some(bytes);
                    break;
                }
                Err(e) => failures.push(format!("{url}: {e}")),
            }
        }
        let Some(bytes) = found else {
            return Err(io::Error::other(format!(
                "download failed: {}",
                failures.join("; ")
            )));
        };
        let tmp = target.with_extension("part");
        fs::write(&tmp, &bytes)?;
        fs::rename(&tmp, &target)?;
        done += model.size;
    }
    Ok(())
}

/// Downloads one copy of `model` from `url`; only the expected file is
/// returned.
fn fetch(model: &ModelFile, url: &str, on_progress: &dyn Fn(u64)) -> io::Result<Vec<u8>> {
    let response = ureq::get(url)
        .timeout(std::time::Duration::from_secs(1800))
        .call()
        .map_err(io::Error::other)?;
    let mut reader = response.into_reader().take(model.size + 1);
    let mut bytes = Vec::with_capacity(model.size as usize);
    let mut chunk = vec![0u8; 256 * 1024];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        on_progress(bytes.len() as u64);
    }
    // Never run a file that is not exactly the one we expect.
    if blake3::hash(&bytes).to_hex().as_str() != model.hash {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not the expected file", model.name),
        ));
    }
    Ok(bytes)
}
