use serde::{Deserialize, Serialize};

/// Whether the app may read the photo library.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    pub granted: bool,
    /// iOS: only the photos the user picked.
    #[serde(default)]
    pub limited: bool,
}

/// A photo or video in the phone's library.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    /// The system's id for it, stable while it stays on the phone.
    pub id: String,
    /// When it was taken (ms since 1970).
    pub taken: i64,
    pub video: bool,
}

/// A file written by [`crate::Photos::export`].
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedFile {
    pub path: String,
    /// Its name on the phone, e.g. `IMG_1234.HEIC`.
    pub name: String,
}

#[cfg(mobile)]
#[derive(Deserialize)]
pub(crate) struct Items {
    pub items: Vec<Asset>,
}

#[cfg(mobile)]
#[derive(Deserialize)]
pub(crate) struct Files {
    pub files: Vec<ExportedFile>,
}

#[cfg(mobile)]
#[derive(Deserialize)]
pub(crate) struct Deleted {
    pub deleted: bool,
}

#[cfg(mobile)]
#[derive(Deserialize)]
pub(crate) struct Name {
    pub name: String,
}
