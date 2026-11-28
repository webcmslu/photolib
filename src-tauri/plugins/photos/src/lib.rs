//! The phone's photo library, for the Phototek phone app: list the photos
//! and videos, copy their originals out to send them to a Phototek server,
//! and delete those the server has, to free up space. Swift on iOS (`ios/`),
//! Kotlin on Android (`android/`); on computers every call fails.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

pub use error::{Error, Result};
pub use models::*;

mod error;
#[cfg(mobile)]
mod mobile;
mod models;

#[cfg(mobile)]
pub use mobile::Photos;

#[cfg(desktop)]
pub struct Photos<R: Runtime>(std::marker::PhantomData<fn() -> R>);

#[cfg(desktop)]
impl<R: Runtime> Photos<R> {
    fn unavailable<T>() -> Result<T> {
        Err(Error::Unavailable)
    }
    pub fn request_access(&self) -> Result<Access> {
        Self::unavailable()
    }
    pub fn list(&self) -> Result<Vec<Asset>> {
        Self::unavailable()
    }
    pub fn export(&self, _id: &str, _dir: &str) -> Result<Vec<ExportedFile>> {
        Self::unavailable()
    }
    pub fn delete(&self, _ids: &[String]) -> Result<bool> {
        Self::unavailable()
    }
    pub fn device_name(&self) -> Result<String> {
        Self::unavailable()
    }
}

/// Access to the photo library from the app: `app.photos()`.
pub trait PhotosExt<R: Runtime> {
    fn photos(&self) -> &Photos<R>;
}

impl<R: Runtime, T: Manager<R>> PhotosExt<R> for T {
    fn photos(&self) -> &Photos<R> {
        self.state::<Photos<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("photos")
        .setup(|app, _api| {
            #[cfg(mobile)]
            let photos = mobile::init(app, _api)?;
            #[cfg(desktop)]
            let photos = Photos::<R>(std::marker::PhantomData);
            app.manage(photos);
            Ok(())
        })
        .build()
}
