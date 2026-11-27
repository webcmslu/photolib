//! The one library the server serves: where its index and thumbnails are,
//! and the background scans that keep them up to date.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use phototek_core::org::OrgPaths;
use phototek_core::thumbs::ThumbCache;
use phototek_core::{db, faces, indexer, org, people, similar, thumbs};

pub struct Library {
    root: PathBuf,
    data_dir: PathBuf,
    thumbs: ThumbCache,
    /// The server's id in the library's shared logs.
    device: String,
    /// Whether faces are searched (`PHOTOTEK_FACES=1`).
    faces: bool,
    face_finder: Mutex<Option<Arc<faces::FaceFinder>>>,
    scanning: AtomicBool,
    /// Set to ask for a scan now; the scan thread waits on it between scans.
    wanted: (Mutex<bool>, Condvar),
    /// Limits on-demand image decoding (thumbnails and converted originals).
    on_demand: Limiter,
}

impl Library {
    pub fn open(root: PathBuf, data_dir: PathBuf, faces: bool) -> Result<Self, String> {
        if !root.is_dir() {
            return Err(format!(
                "the photo folder {} can't be found (set PHOTOTEK_LIBRARY)",
                root.display()
            ));
        }
        std::fs::create_dir_all(&data_dir)
            .map_err(|e| format!("can't create {}: {e}", data_dir.display()))?;
        let device = org::device_id(&data_dir).map_err(|e| e.to_string())?;
        // Creates the library's `.photolib` folder the first time, unless
        // the folder is read-only.
        org::read_or_create_meta(&root).map_err(|e| e.to_string())?;
        let library = Self {
            thumbs: ThumbCache::new(&data_dir),
            root,
            data_dir,
            device,
            faces,
            face_finder: Mutex::new(None),
            scanning: AtomicBool::new(false),
            wanted: (Mutex::new(false), Condvar::new()),
            on_demand: Limiter::new(
                std::thread::available_parallelism().map_or(2, |n| n.get().div_ceil(2)),
            ),
        };
        // Makes the index if there is none yet.
        library.index()?;
        Ok(library)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn thumbs(&self) -> &ThumbCache {
        &self.thumbs
    }

    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }

    pub fn index_path(&self) -> PathBuf {
        self.data_dir.join("index.db")
    }

    /// Opens the index for a query.
    pub fn index(&self) -> Result<rusqlite::Connection, String> {
        db::open(&self.index_path()).map_err(|e| e.to_string())
    }

    pub fn photo_count(&self) -> u64 {
        self.index()
            .ok()
            .and_then(|conn| db::count(&conn).ok())
            .unwrap_or(0)
    }

    pub fn scanning(&self) -> bool {
        self.scanning.load(Ordering::Relaxed)
    }

    pub fn faces_enabled(&self) -> bool {
        self.faces && faces::models_present(&self.data_dir)
    }

    /// Asks for a scan as soon as the current one (if any) is over.
    pub fn request_scan(&self) {
        let (wanted, wake) = &self.wanted;
        *wanted.lock().unwrap() = true;
        wake.notify_one();
    }

    /// Where the organization logs are: the library's shared one, and the
    /// server's own while the library is read-only.
    pub fn org_paths(&self) -> OrgPaths {
        OrgPaths::new(&self.root, &self.data_dir.join("local-log"))
    }

    /// True when the server can write its log into the library, so that its
    /// changes reach the desktop apps (false with a read-only mount: they
    /// then stay on the server).
    pub fn writable(&self) -> bool {
        self.org_paths().shared_is_writable(&self.device)
    }

    /// Records changes (favourites, albums, names…) as this device, like the
    /// desktop apps do. The photo files themselves are never touched.
    pub fn record(&self, conn: &mut rusqlite::Connection, ops: Vec<org::Op>) -> Result<(), String> {
        org::record(conn, &self.org_paths(), &self.device, self.writable(), ops)
            .map_err(|e| e.to_string())
    }

    /// Reads the changes the other devices made since the last time.
    pub fn sync(&self, conn: &mut rusqlite::Connection) -> Result<bool, String> {
        org::sync(conn, &self.org_paths(), &self.device, self.writable())
            .map(|read| read > 0)
            .map_err(|e| e.to_string())
    }

    /// Lets at most a few decodings run at a time.
    pub fn decoding(&self) -> Permit<'_> {
        self.on_demand.acquire()
    }

    /// The face models, loaded the first time (and downloaded if needed).
    fn face_finder(&self) -> Option<Arc<faces::FaceFinder>> {
        if !self.faces {
            return None;
        }
        let mut slot = self.face_finder.lock().unwrap();
        if slot.is_none() {
            if !faces::models_present(&self.data_dir) {
                println!("Downloading the face models…");
                if let Err(e) = faces::download_models(&self.data_dir, &|_| {}) {
                    eprintln!("Couldn't download the face models: {e}");
                    return None;
                }
            }
            *slot = faces::FaceFinder::load(&self.data_dir).ok().map(Arc::new);
        }
        slot.clone()
    }

    /// Merges the organization logs, indexes the folder, makes the missing
    /// thumbnails, then finds similar shots and (if switched on) faces.
    fn scan(&self) -> Result<(), indexer::ScanError> {
        let cancel = AtomicBool::new(false);
        let quiet = |_| {};
        let mut conn = db::open(&self.index_path())?;
        org::sync(&mut conn, &self.org_paths(), &self.device, self.writable())?;
        let summary = indexer::scan_with(
            &self.root,
            &mut conn,
            indexer::Mode::Changes,
            &cancel,
            &quiet,
        )?;
        let made = thumbs::generate_missing(&self.root, &mut conn, &self.thumbs, &cancel, &quiet)?;
        if made.undecodable > 0 {
            db::pair_parts(&mut conn)?;
        }
        similar::hash_pending(&mut conn, &self.thumbs, &cancel, &quiet)?;
        if let Some(finder) = self.face_finder() {
            people::find_faces(&mut conn, &self.thumbs, &finder, &cancel, &quiet)?;
            people::group_faces(&mut conn)?;
        }
        println!(
            "Scan done: {} photos ({} new, {} changed, {} removed).",
            db::count(&conn).unwrap_or(0),
            summary.added,
            summary.updated,
            summary.removed
        );
        Ok(())
    }
}

/// Scans at start, then every `minutes` (never if 0), and whenever asked.
/// With `None`, only when asked.
pub fn start_scans(library: Arc<Library>, minutes: Option<u64>) {
    let mut first = minutes.is_some();
    let minutes = minutes.unwrap_or(0);
    std::thread::spawn(move || loop {
        if !std::mem::take(&mut first) {
            let (wanted, wake) = &library.wanted;
            let mut asked = wanted.lock().unwrap();
            if minutes == 0 {
                while !*asked {
                    asked = wake.wait(asked).unwrap();
                }
            } else {
                let (now, _) = wake
                    .wait_timeout_while(asked, Duration::from_secs(minutes * 60), |asked| !*asked)
                    .unwrap();
                asked = now;
            }
            *asked = false;
        }
        library.scanning.store(true, Ordering::Relaxed);
        if let Err(e) = library.scan() {
            eprintln!("Scan failed: {e}");
        }
        library.scanning.store(false, Ordering::Relaxed);
    });
}

/// Lets at most `permits` threads through at a time.
struct Limiter {
    available: Mutex<usize>,
    freed: Condvar,
}

pub struct Permit<'a>(&'a Limiter);

impl Limiter {
    fn new(permits: usize) -> Self {
        Self {
            available: Mutex::new(permits),
            freed: Condvar::new(),
        }
    }

    fn acquire(&self) -> Permit<'_> {
        let mut available = self.available.lock().unwrap();
        while *available == 0 {
            available = self.freed.wait(available).unwrap();
        }
        *available -= 1;
        Permit(self)
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *self.0.available.lock().unwrap() += 1;
        self.0.freed.notify_one();
    }
}
