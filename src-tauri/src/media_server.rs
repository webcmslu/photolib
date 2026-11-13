//! A tiny HTTP server for video files, used on Linux only.
//!
//! The Linux webview (WebKitGTK) plays video through GStreamer, which can
//! only load from `http://` addresses, not from the app's own `photo://`
//! scheme. So on Linux the viewer gets its videos from this server instead.
//!
//! It listens on the loopback address only (nothing outside this computer can
//! reach it) on a port chosen by the system, and every URL must contain a
//! random token made at startup, so other programs on the computer can't use
//! it to read files either.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Finds the video file for the last part of a request path. `None` if it
/// names nothing that may be served.
pub type Resolver = dyn Fn(&str) -> Option<PathBuf> + Send + Sync;

const MAX_REQUEST_HEAD: u64 = 16 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Starts the server. Returns the base URL: a video is at `<base>/<name>`,
/// where `name` is whatever `resolve` understands.
pub fn start(resolve: Arc<Resolver>) -> io::Result<String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let port = listener.local_addr()?.port();
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
    let mut token = String::new();
    for byte in random {
        token.push_str(&format!("{byte:02x}"));
    }

    let prefix = format!("/{token}/");
    std::thread::Builder::new()
        .name("media-server".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let resolve = Arc::clone(&resolve);
                let prefix = prefix.clone();
                // One thread per connection: a player opens only a few.
                let _ = std::thread::Builder::new()
                    .name("media-connection".into())
                    .spawn(move || {
                        let _ = serve(stream, &prefix, &*resolve);
                    });
            }
        })?;
    Ok(format!("http://127.0.0.1:{port}/{token}"))
}

/// Answers the requests of one connection until the client closes it.
fn serve(stream: TcpStream, prefix: &str, resolve: &Resolver) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(Some(IDLE_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut stream = stream;
    while let Some(request) = read_request(&mut reader)? {
        respond(&mut stream, &request, prefix, resolve)?;
    }
    Ok(())
}

struct Request {
    method: String,
    path: String,
    range: Option<String>,
}

/// Reads one request's first line and headers. `None` when the client has
/// closed the connection.
fn read_request(reader: &mut impl BufRead) -> io::Result<Option<Request>> {
    let mut head = reader.take(MAX_REQUEST_HEAD);
    let mut line = String::new();
    if head.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "bad request"));
    };
    let mut request = Request {
        method: method.to_owned(),
        path: path.to_owned(),
        range: None,
    };
    loop {
        line.clear();
        if head.read_line(&mut line)? == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "bad request"));
        }
        let header = line.trim_end();
        if header.is_empty() {
            return Ok(Some(request));
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("range") {
                request.range = Some(value.trim().to_owned());
            }
        }
    }
}

fn respond(
    stream: &mut TcpStream,
    request: &Request,
    prefix: &str,
    resolve: &Resolver,
) -> io::Result<()> {
    let head_only = request.method == "HEAD";
    if request.method != "GET" && !head_only {
        return write_head(stream, "405 Method Not Allowed", &[], 0);
    }
    let file = request
        .path
        .strip_prefix(prefix)
        .and_then(resolve)
        .and_then(|path| Some((File::open(&path).ok()?, path)));
    let Some((mut file, path)) = file else {
        return write_head(stream, "404 Not Found", &[], 0);
    };
    let len = file.metadata()?.len();
    let mime = ("Content-Type", mime_of(&path).to_owned());
    let ranges = ("Accept-Ranges", "bytes".to_owned());

    let (first, last) = match request.range.as_deref() {
        None => {
            write_head(stream, "200 OK", &[mime, ranges], len)?;
            (0, len.saturating_sub(1))
        }
        Some(range) => match parse_range(range, len) {
            Some((first, last)) => {
                let content_range = ("Content-Range", format!("bytes {first}-{last}/{len}"));
                let headers = [mime, ranges, content_range];
                write_head(stream, "206 Partial Content", &headers, last - first + 1)?;
                (first, last)
            }
            None => {
                let headers = [("Content-Range", format!("bytes */{len}"))];
                return write_head(stream, "416 Range Not Satisfiable", &headers, 0);
            }
        },
    };
    if !head_only && len > 0 {
        file.seek(SeekFrom::Start(first))?;
        io::copy(&mut file.take(last - first + 1), stream)?;
    }
    stream.flush()
}

fn write_head(
    stream: &mut TcpStream,
    status: &str,
    headers: &[(&str, String)],
    content_length: u64,
) -> io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Length: {content_length}\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes())
}

pub fn mime_of(video: &Path) -> &'static str {
    match video
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mov") => "video/quicktime",
        Some("3gp") => "video/3gpp",
        _ => "video/mp4",
    }
}

/// Parses a `Range` header (`bytes=0-99`, `bytes=100-`, `bytes=-50`) for a
/// file of `len` bytes into the first and last byte to send. `None` if the
/// range is malformed or outside the file.
pub fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let spec = header.trim().strip_prefix("bytes=")?;
    // Only the first of several ranges is served.
    let (first, last) = spec.split(',').next()?.trim().split_once('-')?;
    let end = len.checked_sub(1)?;
    let (first, last) = match (first.trim(), last.trim()) {
        ("", suffix) => (len.saturating_sub(suffix.parse().ok()?), end),
        (first, "") => (first.parse().ok()?, end),
        (first, last) => (first.parse().ok()?, last.parse::<u64>().ok()?.min(end)),
    };
    (first <= last && first < len).then_some((first, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_headers() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=100-", 1000), Some((100, 999)));
        assert_eq!(parse_range("bytes=-50", 1000), Some((950, 999)));
        assert_eq!(parse_range("bytes=-5000", 1000), Some((0, 999)));
        assert_eq!(parse_range("bytes=900-5000", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=0-0, 5-9", 1000), Some((0, 0)));
        assert_eq!(parse_range("bytes=1000-", 1000), None);
        assert_eq!(parse_range("bytes=9-3", 1000), None);
        assert_eq!(parse_range("bytes=0-", 0), None);
        assert_eq!(parse_range("items=0-9", 1000), None);
        assert_eq!(parse_range("bytes=abc", 1000), None);
    }

    /// Sends one request on a new connection and returns the whole answer.
    fn ask(base: &str, path: &str, extra_header: &str) -> String {
        let address = base.strip_prefix("http://").unwrap();
        let (host, base_path) = address.split_once('/').unwrap();
        let mut stream = TcpStream::connect(host).unwrap();
        write!(
            stream,
            "GET /{base_path}{path} HTTP/1.1\r\nHost: {host}\r\n{extra_header}Connection: close\r\n\r\n"
        )
        .unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    }

    #[test]
    fn serves_whole_files_and_ranges_only_with_the_token() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("clip.mov");
        std::fs::write(&video, "0123456789").unwrap();
        let base = start(Arc::new(move |name: &str| {
            (name == "clip").then(|| video.clone())
        }))
        .unwrap();

        let whole = ask(&base, "/clip", "");
        assert!(whole.starts_with("HTTP/1.1 200 OK\r\n"), "{whole}");
        assert!(whole.contains("Content-Type: video/quicktime\r\n"));
        assert!(whole.contains("Content-Length: 10\r\n"));
        assert!(whole.ends_with("\r\n\r\n0123456789"));

        let part = ask(&base, "/clip", "Range: bytes=2-5\r\n");
        assert!(
            part.starts_with("HTTP/1.1 206 Partial Content\r\n"),
            "{part}"
        );
        assert!(part.contains("Content-Range: bytes 2-5/10\r\n"));
        assert!(part.ends_with("\r\n\r\n2345"));

        let beyond = ask(&base, "/clip", "Range: bytes=50-\r\n");
        assert!(beyond.starts_with("HTTP/1.1 416 "), "{beyond}");

        assert!(ask(&base, "/other", "").starts_with("HTTP/1.1 404 "));
        // Without the right token, nothing is served.
        let (server, _token) = base.rsplit_once('/').unwrap();
        let wrong = format!("{server}/00000000000000000000000000000000");
        assert!(ask(&wrong, "/clip", "").starts_with("HTTP/1.1 404 "));
    }
}
