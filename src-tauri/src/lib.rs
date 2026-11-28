//! Phototek: the desktop app, where the library is a folder on the computer
//! (`desktop`), and the phone app, which shows a Phototek server's library
//! (`mobile`). Both run the same interface.

#[cfg(desktop)]
mod desktop;
#[cfg(desktop)]
pub use desktop::run;

#[cfg(mobile)]
mod mobile;
#[cfg(mobile)]
pub use mobile::run;
#[cfg(any(mobile, test))]
mod phone_fingerprint;

// The desktop app's modules. They reach the library code as `crate::db` and
// the like.
#[cfg(desktop)]
use phototek_core::{db, fingerprint, indexer, metadata, org, thumbs};
#[cfg(desktop)]
mod apple;
#[cfg(target_os = "macos")]
mod camera_macos;
#[cfg(windows)]
mod camera_windows;
/// Phones and cameras connected by USB, where the system has a way to read them.
#[cfg(target_os = "macos")]
use camera_macos as camera;
#[cfg(windows)]
use camera_windows as camera;
#[cfg(desktop)]
mod importer;
#[cfg(desktop)]
mod recents;
#[cfg(desktop)]
mod rename;
