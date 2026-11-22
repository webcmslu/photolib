//! Phones and cameras connected by USB on Windows, where they don't show up
//! as drives: iPhones (with Apple's driver, from the "Apple Devices" app or
//! iTunes), Android phones in "File transfer" (MTP) or "Transfer photos"
//! (PTP) mode, and cameras. Uses Windows Portable Devices (WPD).
//!
//! Same functions as camera_macos.rs: files are named by their position in
//! the last [`files`] listing of the device.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use chrono::{Local, TimeZone};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Devices::PortableDevices::{
    IPortableDevice, IPortableDeviceContent, IPortableDeviceKeyCollection, IPortableDeviceManager,
    IPortableDevicePropVariantCollection, IPortableDeviceValues, PortableDeviceFTM,
    PortableDeviceKeyCollection, PortableDeviceManager, PortableDevicePropVariantCollection,
    PortableDeviceValues, PORTABLE_DEVICE_DELETE_NO_RECURSION, WPD_CLIENT_MAJOR_VERSION,
    WPD_CLIENT_MINOR_VERSION, WPD_CLIENT_NAME, WPD_CLIENT_REVISION, WPD_CONTENT_TYPE_FOLDER,
    WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT, WPD_DEVICE_OBJECT_ID, WPD_DEVICE_PROTOCOL,
    WPD_OBJECT_CONTENT_TYPE, WPD_OBJECT_DATE_CREATED, WPD_OBJECT_DATE_MODIFIED, WPD_OBJECT_NAME,
    WPD_OBJECT_ORIGINAL_FILE_NAME, WPD_OBJECT_SIZE, WPD_RESOURCE_DEFAULT, WPD_RESOURCE_THUMBNAIL,
};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IStream, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Variant::{VT_DATE, VT_ERROR, VT_FILETIME, VT_LPWSTR};

#[derive(Debug, Clone)]
pub struct Camera {
    pub id: String,
    pub name: String,
    /// Locked, or "Trust" / "Allow" not tapped yet: the device shows no storage.
    pub restricted: bool,
}

#[derive(Debug, Clone)]
pub struct CameraFile {
    pub name: String,
    /// The folder on the device, such as `100APPLE` or `Camera`.
    pub folder: String,
    pub size: u64,
    /// Milliseconds since 1970.
    pub created: Option<i64>,
    pub exif: Option<i64>,
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
                "Unlock the device and tap “Trust” or “Allow” on it, then try again."
            ),
            Self::Other(why) => write!(f, "{why}"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Deleted {
    /// The files deleted, and those the device refused to delete (with why).
    pub deleted: Vec<usize>,
    pub failed: Vec<(usize, String)>,
}

/// A file of the last listing of a device.
#[derive(Debug, Clone)]
struct Listed {
    object: String,
    name: String,
    size: u64,
}

/// The last listing of each device, by device id.
static CATALOGS: Mutex<Option<HashMap<String, Vec<Listed>>>> = Mutex::new(None);

/// The device opened last (opening one takes a moment; thumbnails come one
/// by one). WPD objects made through `PortableDeviceFTM` can be used from
/// any thread.
struct Opened(String, IPortableDevice);
// SAFETY: created with the free-threaded marshaler (PortableDeviceFTM).
unsafe impl Send for Opened {}
static OPENED: Mutex<Option<Opened>> = Mutex::new(None);

fn error(e: windows::core::Error) -> String {
    e.message().trim().to_string()
}

/// COM for this thread (once per thread is enough; never undone, since the
/// device objects outlive the call).
fn com() {
    // SAFETY: plain call; "already initialized" (even in another mode) is fine.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
}

/// Takes a string the device allocated.
fn take(text: PWSTR) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: a NUL-terminated string from CoTaskMemAlloc, freed right after.
    let owned = unsafe { text.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(text.0 as *const _)) };
    owned
}

fn manager() -> windows::core::Result<IPortableDeviceManager> {
    // SAFETY: plain COM creation.
    unsafe { CoCreateInstance(&PortableDeviceManager, None, CLSCTX_INPROC_SERVER) }
}

/// The plug-and-play ids of the portable devices connected right now.
fn device_ids(manager: &IPortableDeviceManager) -> Vec<String> {
    let mut count = 0u32;
    // SAFETY: the first call gives the count, the second fills `ids`.
    unsafe {
        let _ = manager.RefreshDeviceList();
        if manager
            .GetDevices(std::ptr::null_mut(), &mut count)
            .is_err()
            || count == 0
        {
            return Vec::new();
        }
        let mut ids = vec![PWSTR::null(); count as usize];
        if manager.GetDevices(ids.as_mut_ptr(), &mut count).is_err() {
            return Vec::new();
        }
        ids.truncate(count as usize);
        ids.into_iter().map(take).collect()
    }
}

fn friendly_name(manager: &IPortableDeviceManager, id: &str) -> String {
    let id = HSTRING::from(id);
    let mut length = 0u32;
    // SAFETY: the first call gives the length, the second fills `buffer`.
    unsafe {
        if manager
            .GetDeviceFriendlyName(&id, PWSTR::null(), &mut length)
            .is_err()
            || length == 0
        {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize];
        if manager
            .GetDeviceFriendlyName(&id, PWSTR(buffer.as_mut_ptr()), &mut length)
            .is_err()
        {
            return String::new();
        }
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        String::from_utf16_lossy(&buffer[..end])
    }
}

fn open_device(id: &str) -> windows::core::Result<IPortableDevice> {
    // SAFETY: plain COM calls on objects created here.
    unsafe {
        let client: IPortableDeviceValues =
            CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER)?;
        client.SetStringValue(&WPD_CLIENT_NAME, &HSTRING::from("Photolib"))?;
        client.SetUnsignedIntegerValue(&WPD_CLIENT_MAJOR_VERSION, 1)?;
        client.SetUnsignedIntegerValue(&WPD_CLIENT_MINOR_VERSION, 0)?;
        client.SetUnsignedIntegerValue(&WPD_CLIENT_REVISION, 0)?;
        let device: IPortableDevice =
            CoCreateInstance(&PortableDeviceFTM, None, CLSCTX_INPROC_SERVER)?;
        device.Open(&HSTRING::from(id), &client)?;
        Ok(device)
    }
}

/// The device `id`, opened (the last one opened is kept open).
fn device(id: &str) -> Result<IPortableDevice, String> {
    com();
    let mut opened = OPENED.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(Opened(last, device)) = opened.as_ref() {
        if last == id {
            return Ok(device.clone());
        }
    }
    let device = open_device(id).map_err(error)?;
    if let Some(Opened(_, old)) = opened.take() {
        // SAFETY: plain COM call.
        let _ = unsafe { old.Close() };
    }
    *opened = Some(Opened(id.to_string(), device.clone()));
    Ok(device)
}

/// Forgets the open device (it was unplugged, or failed).
fn forget(id: &str) {
    let mut opened = OPENED.lock().unwrap_or_else(|e| e.into_inner());
    if opened.as_ref().is_some_and(|Opened(last, _)| last == id) {
        *opened = None;
    }
}

/// The ids of the objects in `parent`.
fn children(content: &IPortableDeviceContent, parent: &str) -> Vec<String> {
    let mut found = Vec::new();
    // SAFETY: plain COM calls; each id is freed by `take`.
    unsafe {
        let Ok(objects) = content.EnumObjects(0, &HSTRING::from(parent), None) else {
            return found;
        };
        loop {
            let mut batch = [PWSTR::null(); 64];
            let mut fetched = 0u32;
            let result = objects.Next(&mut batch, &mut fetched);
            for id in batch.into_iter().take(fetched as usize) {
                found.push(take(id));
            }
            if result.is_err() || fetched == 0 || result.0 == 1 {
                break; // S_FALSE: the end
            }
        }
    }
    found
}

/// What a listing needs to know of an object.
struct Properties {
    name: String,
    folder: bool,
    container: bool,
    size: u64,
    created: Option<i64>,
}

fn keys(keys: &[PROPERTYKEY]) -> windows::core::Result<IPortableDeviceKeyCollection> {
    // SAFETY: plain COM calls.
    unsafe {
        let collection: IPortableDeviceKeyCollection =
            CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER)?;
        for key in keys {
            collection.Add(key)?;
        }
        Ok(collection)
    }
}

/// A date of an object, in milliseconds since 1970.
fn date(values: &IPortableDeviceValues, key: &PROPERTYKEY) -> Option<i64> {
    // SAFETY: the PROPVARIANT is read according to its type, then cleared.
    unsafe {
        let mut value = values.GetValue(key).ok()?;
        let inner = &value.Anonymous.Anonymous;
        let ms = if inner.vt == VT_DATE {
            // Days since 1899-12-30, in the device's (local) time.
            let days = inner.Anonymous.date;
            let wall = ((days - 25_569.0) * 86_400_000.0).round() as i64;
            let naive = chrono::DateTime::from_timestamp_millis(wall)?.naive_utc();
            Local
                .from_local_datetime(&naive)
                .earliest()
                .map(|when| when.timestamp_millis())
        } else if inner.vt == VT_FILETIME {
            let ft = inner.Anonymous.filetime;
            let ticks = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
            Some((ticks as i64 - 116_444_736_000_000_000) / 10_000)
        } else {
            None
        };
        let _ = PropVariantClear(&mut value);
        ms.filter(|&ms| ms > 0)
    }
}

fn properties(
    content: &IPortableDeviceContent,
    wanted: &IPortableDeviceKeyCollection,
    object: &str,
) -> Option<Properties> {
    // SAFETY: plain COM calls; strings are freed by `take`.
    unsafe {
        let values = content
            .Properties()
            .ok()?
            .GetValues(&HSTRING::from(object), wanted)
            .ok()?;
        let kind = values.GetGuidValue(&WPD_OBJECT_CONTENT_TYPE).ok();
        let mut name = values
            .GetStringValue(&WPD_OBJECT_ORIGINAL_FILE_NAME)
            .map(take)
            .unwrap_or_default();
        if name.is_empty() {
            name = values
                .GetStringValue(&WPD_OBJECT_NAME)
                .map(take)
                .unwrap_or_default();
        }
        Some(Properties {
            name,
            folder: kind == Some(WPD_CONTENT_TYPE_FOLDER),
            container: kind == Some(WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT),
            size: values
                .GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE)
                .unwrap_or(0),
            created: date(&values, &WPD_OBJECT_DATE_CREATED)
                .or_else(|| date(&values, &WPD_OBJECT_DATE_MODIFIED)),
        })
    }
}

fn listing_keys() -> windows::core::Result<IPortableDeviceKeyCollection> {
    keys(&[
        WPD_OBJECT_CONTENT_TYPE,
        WPD_OBJECT_ORIGINAL_FILE_NAME,
        WPD_OBJECT_NAME,
        WPD_OBJECT_SIZE,
        WPD_OBJECT_DATE_CREATED,
        WPD_OBJECT_DATE_MODIFIED,
    ])
}

/// The `DCIM` folders of the device's storages ("Internal storage", "SD card"),
/// and whether it has any storage at all (a locked phone shows none).
fn dcim_folders(
    content: &IPortableDeviceContent,
    wanted: &IPortableDeviceKeyCollection,
) -> (Vec<String>, bool) {
    let mut folders = Vec::new();
    let mut storages = 0;
    for storage in children(content, &device_object_id()) {
        let Some(props) = properties(content, wanted, &storage) else {
            continue;
        };
        if !props.container && !props.folder {
            continue;
        }
        storages += 1;
        for child in children(content, &storage) {
            if properties(content, wanted, &child)
                .is_some_and(|p| p.folder && p.name.eq_ignore_ascii_case("DCIM"))
            {
                folders.push(child);
            }
        }
    }
    (folders, storages > 0)
}

fn device_object_id() -> String {
    // SAFETY: a static wide string.
    unsafe { WPD_DEVICE_OBJECT_ID.to_string() }.unwrap_or_else(|_| "DEVICE".into())
}

/// The protocol a device speaks ("MTP: 1.00", "PTP: 1.00", "MSC:").
fn protocol(content: &IPortableDeviceContent) -> String {
    // SAFETY: plain COM calls.
    unsafe {
        let Ok(wanted) = keys(&[WPD_DEVICE_PROTOCOL]) else {
            return String::new();
        };
        content
            .Properties()
            .and_then(|p| p.GetValues(&HSTRING::from(device_object_id()), &wanted))
            .and_then(|values| values.GetStringValue(&WPD_DEVICE_PROTOCOL))
            .map(take)
            .unwrap_or_default()
    }
}

/// The phones and cameras connected right now. Drives (memory cards, which
/// WPD lists too) are left out: they are found as drives.
pub fn cameras() -> Vec<Camera> {
    com();
    let Ok(manager) = manager() else {
        return Vec::new();
    };
    let Ok(wanted) = listing_keys() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for id in device_ids(&manager) {
        let Ok(device) = device(&id) else {
            forget(&id);
            continue;
        };
        // SAFETY: plain COM call.
        let Ok(content) = (unsafe { device.Content() }) else {
            forget(&id);
            continue;
        };
        if protocol(&content).to_ascii_uppercase().starts_with("MSC") {
            continue;
        }
        let (folders, any_storage) = dcim_folders(&content, &wanted);
        if folders.is_empty() && any_storage {
            continue; // a music player, say
        }
        let name = friendly_name(&manager, &id);
        found.push(Camera {
            name: if name.is_empty() {
                "Phone".into()
            } else {
                name
            },
            restricted: !any_storage,
            id,
        });
    }
    found
}

/// Lists the photos and videos of the device (everything under its `DCIM`
/// folders); their positions in the list are how the other functions name them.
pub fn files(device_id: &str) -> Result<Vec<CameraFile>, CameraError> {
    let device = device(device_id).map_err(CameraError::Other)?;
    // SAFETY: plain COM call.
    let content = unsafe { device.Content() }.map_err(|e| {
        forget(device_id);
        CameraError::Other(error(e))
    })?;
    let wanted = listing_keys().map_err(|e| CameraError::Other(error(e)))?;
    let (folders, any_storage) = dcim_folders(&content, &wanted);
    if !any_storage {
        return Err(CameraError::Restricted);
    }
    let mut listed = Vec::new();
    let mut files = Vec::new();
    // (object, the name of its folder)
    let mut pending: Vec<(String, String)> =
        folders.into_iter().map(|f| (f, "DCIM".into())).collect();
    while let Some((folder, folder_name)) = pending.pop() {
        for child in children(&content, &folder) {
            let Some(props) = properties(&content, &wanted, &child) else {
                continue;
            };
            if props.folder {
                pending.push((child, props.name));
                continue;
            }
            listed.push(Listed {
                object: child,
                name: props.name.clone(),
                size: props.size,
            });
            files.push(CameraFile {
                name: props.name,
                folder: folder_name.clone(),
                size: props.size,
                created: props.created,
                exif: None,
            });
        }
    }
    CATALOGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(device_id.to_string(), listed);
    Ok(files)
}

fn listed(device_id: &str, index: usize) -> Result<Listed, String> {
    CATALOGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|catalogs| catalogs.get(device_id))
        .and_then(|files| files.get(index))
        .cloned()
        .ok_or_else(|| "Look at the device's photos again first.".to_string())
}

/// A stream of one resource (the file itself, or its thumbnail) of an object.
fn stream(
    device: &IPortableDevice,
    object: &str,
    resource: &PROPERTYKEY,
) -> windows::core::Result<(IStream, usize)> {
    // SAFETY: plain COM calls; `stream` is set on success.
    unsafe {
        let resources = device.Content()?.Transfer()?;
        let mut optimal = 0u32;
        let mut stream: Option<IStream> = None;
        resources.GetStream(
            &HSTRING::from(object),
            resource,
            STGM_READ.0,
            &mut optimal,
            &mut stream,
        )?;
        let stream =
            stream.ok_or_else(|| windows::core::Error::from_hresult(windows::core::HRESULT(-1)))?;
        Ok((stream, (optimal as usize).clamp(64 * 1024, 4 << 20)))
    }
}

/// Reads a stream chunk by chunk; `false` from `each` stops.
fn read_all(
    stream: &IStream,
    chunk: usize,
    mut each: impl FnMut(&[u8]) -> std::io::Result<bool>,
) -> Result<bool, String> {
    let mut buffer = vec![0u8; chunk];
    loop {
        let mut read = 0u32;
        // SAFETY: `buffer` holds `chunk` bytes.
        let result = unsafe {
            stream.Read(
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                Some(&mut read),
            )
        };
        if result.is_err() {
            return Err(error(windows::core::Error::from_hresult(result)));
        }
        if read == 0 {
            return Ok(true);
        }
        if !each(&buffer[..read as usize]).map_err(|e| e.to_string())? {
            return Ok(false);
        }
    }
}

/// Downloads file `index` into `dir` as `name`.
pub fn download(
    device_id: &str,
    index: usize,
    dir: &Path,
    name: &str,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let file = listed(device_id, index)?;
    let device = device(device_id)?;
    let (stream, chunk) = stream(&device, &file.object, &WPD_RESOURCE_DEFAULT).map_err(|e| {
        forget(device_id);
        error(e)
    })?;
    let target = dir.join(name);
    let mut out = File::create(&target).map_err(|e| e.to_string())?;
    let finished = read_all(&stream, chunk, |bytes| {
        if cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        out.write_all(bytes)?;
        Ok(true)
    });
    match finished {
        Ok(true) => out.sync_all().map_err(|e| e.to_string()),
        Ok(false) => {
            drop(out);
            let _ = std::fs::remove_file(&target);
            Err("cancelled".into())
        }
        Err(why) => {
            drop(out);
            let _ = std::fs::remove_file(&target);
            Err(why)
        }
    }
}

/// The thumbnail the device keeps for file `index` (usually a small JPEG).
pub fn thumbnail(device_id: &str, index: usize, _max_side: u32) -> Result<Vec<u8>, String> {
    let file = listed(device_id, index)?;
    let device = device(device_id)?;
    let (stream, chunk) = stream(&device, &file.object, &WPD_RESOURCE_THUMBNAIL).map_err(error)?;
    let mut bytes = Vec::new();
    read_all(&stream, chunk, |part| {
        bytes.extend_from_slice(part);
        // A thumbnail is small; anything bigger is not one.
        Ok(bytes.len() < 4 << 20)
    })?;
    if bytes.is_empty() {
        return Err("no thumbnail".into());
    }
    Ok(bytes)
}

/// A PROPVARIANT holding a string (owned by `text`, which must outlive it).
fn string_variant(text: &[u16]) -> PROPVARIANT {
    let mut value = PROPVARIANT::default();
    // SAFETY: setting the type and the matching field of the union.
    unsafe {
        let inner = &mut *value.Anonymous.Anonymous;
        inner.vt = VT_LPWSTR;
        inner.Anonymous.pwszVal = PWSTR(text.as_ptr() as *mut u16);
    }
    value
}

/// Deletes files from the device, given as (index, name, size): a file is
/// only deleted if the one at that index still has that name and size.
pub fn delete(device_id: &str, files: &[(usize, &str, u64)]) -> Result<Deleted, String> {
    let device = device(device_id)?;
    // SAFETY: plain COM call.
    let content = unsafe { device.Content() }.map_err(error)?;
    let wanted = listing_keys().map_err(error)?;
    let mut result = Deleted::default();
    let mut checked: Vec<(usize, Vec<u16>)> = Vec::new();
    for &(index, name, size) in files {
        let still = listed(device_id, index).ok().filter(|file| {
            file.name == name
                && file.size == size
                && properties(&content, &wanted, &file.object)
                    .is_some_and(|now| now.name == name && now.size == size)
        });
        match still {
            Some(file) => checked.push((
                index,
                file.object
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect(),
            )),
            None => result
                .failed
                .push((index, "changed on the device since it was looked at".into())),
        }
    }
    for batch in checked.chunks(100) {
        delete_batch(&content, batch, &mut result);
    }
    Ok(result)
}

fn delete_batch(
    content: &IPortableDeviceContent,
    batch: &[(usize, Vec<u16>)],
    result: &mut Deleted,
) {
    // SAFETY: the collection copies each string; `batch` outlives the calls.
    let outcome = unsafe {
        (|| -> windows::core::Result<Option<IPortableDevicePropVariantCollection>> {
            let objects: IPortableDevicePropVariantCollection = CoCreateInstance(
                &PortableDevicePropVariantCollection,
                None,
                CLSCTX_INPROC_SERVER,
            )?;
            for (_, object) in batch {
                let value = string_variant(object);
                objects.Add(&value)?;
            }
            let mut results: Option<IPortableDevicePropVariantCollection> = None;
            let deleted = content.Delete(
                PORTABLE_DEVICE_DELETE_NO_RECURSION.0 as u32,
                &objects,
                &mut results,
            );
            match (deleted, results) {
                (Ok(()), results) => Ok(results),
                // Some failed: the results say which.
                (Err(_), Some(results)) => Ok(Some(results)),
                (Err(e), None) => Err(e),
            }
        })()
    };
    match outcome {
        Err(e) => {
            let why = error(e);
            result
                .failed
                .extend(batch.iter().map(|(index, _)| (*index, why.clone())));
        }
        Ok(None) => result.deleted.extend(batch.iter().map(|(index, _)| *index)),
        Ok(Some(results)) => {
            for (position, (index, _)) in batch.iter().enumerate() {
                let mut value = PROPVARIANT::default();
                // SAFETY: GetAt fills `value`, read according to its type, then cleared.
                let code = unsafe {
                    let code = results
                        // Declared `*const`, but it fills the value in.
                        .GetAt(position as u32, std::ptr::addr_of_mut!(value).cast_const())
                        .ok()
                        .and_then(|()| {
                            let inner = &value.Anonymous.Anonymous;
                            (inner.vt == VT_ERROR).then(|| inner.Anonymous.scode)
                        });
                    let _ = PropVariantClear(&mut value);
                    code
                };
                match code {
                    Some(code) if code < 0 => result.failed.push((
                        *index,
                        error(windows::core::Error::from_hresult(windows::core::HRESULT(
                            code,
                        ))),
                    )),
                    _ => result.deleted.push(*index),
                }
            }
        }
    }
}
