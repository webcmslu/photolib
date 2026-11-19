//! iPhones, iPads and cameras connected by USB on macOS, where they don't
//! show up as drives. Wraps the ImageCaptureCore bridge in camera_macos.m.

use std::ffi::{c_char, c_long, CStr, CString};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use serde::Deserialize;

extern "C" {
    fn pl_camera_free(text: *mut c_char);
    fn pl_camera_list() -> *mut c_char;
    fn pl_camera_files(device: *const c_char, timeout: f64) -> *mut c_char;
    fn pl_camera_read(
        device: *const c_char,
        index: c_long,
        offset: i64,
        length: i64,
        out: *mut u8,
        error: *mut *mut c_char,
    ) -> i64;
    fn pl_camera_thumbnail(
        device: *const c_char,
        index: c_long,
        max_side: c_long,
        out: *mut *mut u8,
        error: *mut *mut c_char,
    ) -> i64;
    fn pl_camera_download(
        device: *const c_char,
        index: c_long,
        dir: *const c_char,
        name: *const c_char,
        cancel: *const AtomicI32,
    ) -> *mut c_char;
}

/// Takes a string from the bridge.
fn take(text: *mut c_char) -> Option<String> {
    if text.is_null() {
        return None;
    }
    // SAFETY: the bridge returns NUL-terminated strings from malloc, which
    // `pl_camera_free` releases.
    let owned = unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned();
    unsafe { pl_camera_free(text) };
    Some(owned)
}

fn c_string(text: &str) -> CString {
    CString::new(text.replace('\0', "")).expect("no NUL left")
}

#[derive(Debug, Clone, Deserialize)]
pub struct Camera {
    pub id: String,
    pub name: String,
    /// Locked, or "Trust this computer" not answered yet.
    pub restricted: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CameraFile {
    pub name: String,
    /// The folder on the device, such as `100APPLE`.
    pub folder: String,
    pub size: u64,
    /// Milliseconds since 1970.
    pub created: Option<i64>,
    pub exif: Option<i64>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Answer<T> {
    Error { error: String },
    Value(T),
}

/// Why the files of a device can't be listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraError {
    /// Locked, or the computer is not trusted yet.
    Restricted,
    Other(String),
}

impl std::fmt::Display for CameraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Restricted => write!(
                f,
                "Unlock the device and tap “Trust” on it, then try again."
            ),
            Self::Other(why) => write!(f, "{why}"),
        }
    }
}

/// The devices connected right now. The first call starts watching for
/// them, so a device may only show up from the next call on.
pub fn cameras() -> Vec<Camera> {
    // SAFETY: plain C call returning a JSON string.
    take(unsafe { pl_camera_list() })
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

/// Opens the device and lists its photos and videos; their positions in the
/// list are how [`read`] and [`download`] name them.
pub fn files(device: &str) -> Result<Vec<CameraFile>, CameraError> {
    let id = c_string(device);
    // A phone with many photos takes a while to list them all.
    // SAFETY: plain C call with a valid string, returning a JSON string.
    let json = take(unsafe { pl_camera_files(id.as_ptr(), 600.0) })
        .ok_or_else(|| CameraError::Other("no answer".into()))?;
    match serde_json::from_str::<Answer<Vec<CameraFile>>>(&json) {
        Ok(Answer::Value(files)) => Ok(files),
        Ok(Answer::Error { error }) if error == "restricted" => Err(CameraError::Restricted),
        Ok(Answer::Error { error }) => Err(CameraError::Other(error)),
        Err(e) => Err(CameraError::Other(e.to_string())),
    }
}

/// `length` bytes at `offset` of file `index`.
pub fn read(device: &str, index: usize, offset: u64, length: usize) -> Result<Vec<u8>, String> {
    let id = c_string(device);
    let mut buffer = vec![0u8; length];
    let mut error: *mut c_char = std::ptr::null_mut();
    // SAFETY: `buffer` holds `length` bytes, which is all the bridge writes.
    let got = unsafe {
        pl_camera_read(
            id.as_ptr(),
            index as c_long,
            offset as i64,
            length as i64,
            buffer.as_mut_ptr(),
            &mut error,
        )
    };
    let error = take(error);
    if got < 0 {
        return Err(error.unwrap_or_else(|| "can't read".into()));
    }
    buffer.truncate(got as usize);
    Ok(buffer)
}

/// Downloads file `index` into `dir` as `name`.
pub fn download(
    device: &str,
    index: usize,
    dir: &Path,
    name: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let id = c_string(device);
    let dir = c_string(&dir.to_string_lossy());
    let name = c_string(name);
    // The bridge watches a C int; a thread copies the Rust flag into it.
    let flag = AtomicI32::new(0);
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            while !finished.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    flag.store(1, Ordering::Relaxed);
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
        // SAFETY: valid strings, and `flag` outlives the call.
        let error = take(unsafe {
            pl_camera_download(
                id.as_ptr(),
                index as c_long,
                dir.as_ptr(),
                name.as_ptr(),
                &flag,
            )
        });
        finished.store(true, Ordering::Relaxed);
        error.map_or(Ok(()), Err)
    })
}

/// A small JPEG of file `index`, made by the device (photos and videos).
pub fn thumbnail(device: &str, index: usize, max_side: u32) -> Result<Vec<u8>, String> {
    let id = c_string(device);
    let mut out: *mut u8 = std::ptr::null_mut();
    let mut error: *mut c_char = std::ptr::null_mut();
    // SAFETY: the bridge sets `out` to a malloc'd buffer of the returned length.
    let length = unsafe {
        pl_camera_thumbnail(
            id.as_ptr(),
            index as c_long,
            max_side as c_long,
            &mut out,
            &mut error,
        )
    };
    let error = take(error);
    if length < 0 || out.is_null() {
        return Err(error.unwrap_or_else(|| "no thumbnail".into()));
    }
    // SAFETY: `out` holds `length` bytes, released right after the copy.
    let bytes = unsafe { std::slice::from_raw_parts(out, length as usize) }.to_vec();
    unsafe { pl_camera_free(out.cast()) };
    Ok(bytes)
}
