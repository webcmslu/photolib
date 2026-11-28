use serde::de::DeserializeOwned;
use serde_json::json;
use tauri::{
    plugin::{PluginApi, PluginHandle},
    AppHandle, Runtime,
};

use crate::models::*;

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_photos);

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    api: PluginApi<R, C>,
) -> crate::Result<Photos<R>> {
    #[cfg(target_os = "android")]
    let handle = api.register_android_plugin("lu.webcms.phototek.photos", "PhotosPlugin")?;
    #[cfg(target_os = "ios")]
    let handle = api.register_ios_plugin(init_plugin_photos)?;
    Ok(Photos(handle))
}

/// The phone's photo library.
pub struct Photos<R: Runtime>(PluginHandle<R>);

impl<R: Runtime> Photos<R> {
    /// Asks for access to the photo library (the system asks the user once).
    pub fn request_access(&self) -> crate::Result<Access> {
        Ok(self.0.run_mobile_plugin("requestAccess", ())?)
    }

    /// Every photo and video, oldest first.
    pub fn list(&self) -> crate::Result<Vec<Asset>> {
        let items: Items = self.0.run_mobile_plugin("list", ())?;
        Ok(items.items)
    }

    /// Writes the originals of a photo or video into `dir` (a Live Photo
    /// gives a photo and a video).
    pub fn export(&self, id: &str, dir: &str) -> crate::Result<Vec<ExportedFile>> {
        let files: Files = self
            .0
            .run_mobile_plugin("export", json!({ "id": id, "dir": dir }))?;
        Ok(files.files)
    }

    /// Deletes photos and videos, once the user confirms; false if they
    /// didn't.
    pub fn delete(&self, ids: &[String]) -> crate::Result<bool> {
        let deleted: Deleted = self.0.run_mobile_plugin("delete", json!({ "ids": ids }))?;
        Ok(deleted.deleted)
    }

    /// Backs up every `hours` while the app is closed, with these settings
    /// (`record` is the app's record of what is saved). False where the
    /// system doesn't allow it yet (iOS).
    pub fn schedule_backup(
        &self,
        url: &str,
        token: Option<&str>,
        device: &str,
        record: &str,
        hours: u32,
    ) -> crate::Result<bool> {
        let answer: Scheduled = self.0.run_mobile_plugin(
            "scheduleBackup",
            json!({ "url": url, "token": token, "device": device, "record": record, "hours": hours }),
        )?;
        Ok(answer.scheduled)
    }

    pub fn cancel_backup(&self) -> crate::Result<()> {
        let _: Scheduled = self.0.run_mobile_plugin("cancelBackup", ())?;
        Ok(())
    }

    /// True on a TV (Android TV).
    pub fn is_television(&self) -> crate::Result<bool> {
        let answer: Television = self.0.run_mobile_plugin("isTelevision", ())?;
        Ok(answer.tv)
    }

    /// The phone's name, as its owner set it.
    pub fn device_name(&self) -> crate::Result<String> {
        let name: Name = self.0.run_mobile_plugin("deviceName", ())?;
        Ok(name.name)
    }
}
