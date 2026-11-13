//! Gets a still picture out of a video, for its thumbnail.
//!
//! The app has no video decoder of its own. It runs a tool that is already on
//! the computer:
//! - `ffmpeg`, if it is installed (looked up on the PATH and in the usual
//!   Homebrew folders, because a macOS app started from the Finder does not
//!   get the shell's PATH);
//! - on macOS, `qlmanage`, the Quick Look tool that ships with the system.
//!
//! Without either, videos are still listed and playable; they just show a
//! placeholder instead of a thumbnail.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use image::RgbImage;

/// The longest side asked for; previews are no larger than this.
const MAX_SIDE: u32 = 1024;
/// A frame grab that takes longer than this is stuck (a stalled network
/// mount, a damaged file).
const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum FrameError {
    /// Nothing on this computer can read videos.
    NoTool,
    /// The tool ran but gave no picture.
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Tool {
    Ffmpeg(PathBuf),
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    QuickLook,
}

fn tool() -> Option<&'static Tool> {
    static TOOL: OnceLock<Option<Tool>> = OnceLock::new();
    TOOL.get_or_init(|| {
        if let Some(ffmpeg) = find_ffmpeg() {
            return Some(Tool::Ffmpeg(ffmpeg));
        }
        cfg!(target_os = "macos").then_some(Tool::QuickLook)
    })
    .as_ref()
}

/// True if video thumbnails can be made on this computer.
pub fn available() -> bool {
    tool().is_some()
}

fn find_ffmpeg() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    let on_path = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    let beside_app = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    let usual = ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"]
        .into_iter()
        .map(PathBuf::from);
    on_path
        .into_iter()
        .chain(beside_app)
        .chain(usual)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Returns one upright frame from near the start of the video, at most
/// `MAX_SIDE` pixels on its long side. `scratch` is a folder for temporary
/// files.
pub fn grab(video: &Path, scratch: &Path) -> Result<RgbImage, FrameError> {
    match tool().ok_or(FrameError::NoTool)? {
        Tool::Ffmpeg(ffmpeg) => {
            // One second in skips the black or blurry first frames. Videos
            // shorter than that give nothing, so fall back to the first frame.
            grab_with_ffmpeg(ffmpeg, video, "1").or_else(|_| grab_with_ffmpeg(ffmpeg, video, "0"))
        }
        Tool::QuickLook => grab_with_quicklook(video, scratch),
    }
}

fn grab_with_ffmpeg(ffmpeg: &Path, video: &Path, seconds: &str) -> Result<RgbImage, FrameError> {
    let mut command = Command::new(ffmpeg);
    command
        .args(["-v", "error", "-nostdin", "-ss", seconds, "-i"])
        .arg(video)
        .args(["-frames:v", "1", "-vf"])
        // Shrink to fit, never enlarge. ffmpeg has already applied the
        // video's rotation at this point.
        .arg(format!(
            "scale='min({MAX_SIDE},iw)':'min({MAX_SIDE},ih)':force_original_aspect_ratio=decrease"
        ))
        .args(["-f", "image2pipe", "-c:v", "mjpeg", "-q:v", "3", "pipe:1"]);
    let jpeg = run(command)?;
    image::load_from_memory(&jpeg)
        .map(|image| image.into_rgb8())
        .map_err(|e| FrameError::Failed(e.to_string()))
}

/// `qlmanage -t` writes `<file name>.png` into the output folder.
fn grab_with_quicklook(video: &Path, scratch: &Path) -> Result<RgbImage, FrameError> {
    let failed = |why: String| FrameError::Failed(why);
    let out_dir = scratch.join(format!(
        "ql-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&out_dir).map_err(|e| failed(e.to_string()))?;

    let mut command = Command::new("/usr/bin/qlmanage");
    command
        .args(["-t", "-s", &MAX_SIDE.to_string(), "-o"])
        .arg(&out_dir)
        .arg(video);
    let result = run(command).and_then(|_| {
        let mut name = video.file_name().unwrap_or_default().to_os_string();
        name.push(".png");
        image::open(out_dir.join(name))
            .map(|image| image.into_rgb8())
            .map_err(|e| failed(e.to_string()))
    });
    let _ = std::fs::remove_dir_all(&out_dir);
    result
}

/// Runs a command to its end and returns what it printed, killing it if it
/// takes longer than `TIMEOUT`.
fn run(mut command: Command) -> Result<Vec<u8>, FrameError> {
    let failed = |why: String| FrameError::Failed(why);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console window flashing up for each video.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| failed(e.to_string()))?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    // Read on another thread, so that a full pipe can't block the tool
    // while we wait for it.
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let _ = stdout.read_to_end(&mut output);
        output
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < TIMEOUT => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(failed("timed out".into()));
            }
            Err(e) => return Err(failed(e.to_string())),
        }
    };
    let output = reader.join().unwrap_or_default();
    if !status.success() {
        return Err(failed(format!("exited with {status}")));
    }
    Ok(output)
}
