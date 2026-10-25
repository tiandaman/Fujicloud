//! The `fuji://` image protocol.
//!
//! The webview loads every thumbnail, preview and original through plain
//! `<img src>` URLs, so decoding happens off the main thread in the browser
//! and nothing is base64'd through IPC. Requests are served from a small
//! worker pool with three lanes:
//!
//! * high   – the photo open in the viewer (FIFO)
//! * normal – grid thumbnails (LIFO: when scrolling fast, what's on screen
//!   *now* is served before what scrolled past a second ago)
//! * low    – background cache warming
//!
//! URLs: `/t/<id>` thumbnail, `/p/<id>` preview, `/o/<id>` full resolution.

use std::collections::{HashSet, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use fuji_core::thumbs::{self, Size};
use fuji_core::{Kind, raf};
use parking_lot::{Condvar, Mutex};
use tauri::http::{Request, Response, StatusCode, header};

use crate::AppState;

type Job = Box<dyn FnOnce() + Send>;

#[derive(Debug, Clone, Copy)]
pub enum Lane {
    High,
    Normal,
    Low,
}

#[derive(Default)]
struct Lanes {
    high: VecDeque<Job>,
    normal: Vec<Job>,
    low: VecDeque<Job>,
}

pub struct Queue {
    lanes: Mutex<Lanes>,
    ready: Condvar,
}

impl Queue {
    pub fn start(workers: usize) -> Arc<Self> {
        let queue = Arc::new(Queue {
            lanes: Mutex::default(),
            ready: Condvar::new(),
        });
        for i in 0..workers.max(2) {
            let q = queue.clone();
            std::thread::Builder::new()
                .name(format!("fuji-img-{i}"))
                .spawn(move || q.work())
                .expect("spawn image worker");
        }
        queue
    }

    pub fn push(&self, lane: Lane, job: impl FnOnce() + Send + 'static) {
        let mut lanes = self.lanes.lock();
        match lane {
            Lane::High => lanes.high.push_back(Box::new(job)),
            Lane::Normal => lanes.normal.push(Box::new(job)),
            Lane::Low => lanes.low.push_back(Box::new(job)),
        }
        drop(lanes);
        self.ready.notify_one();
    }

    pub fn backlog(&self) -> usize {
        let lanes = self.lanes.lock();
        lanes.high.len() + lanes.normal.len() + lanes.low.len()
    }

    fn work(&self) {
        loop {
            let job = {
                let mut lanes = self.lanes.lock();
                loop {
                    if let Some(job) = lanes
                        .high
                        .pop_front()
                        .or_else(|| lanes.normal.pop())
                        .or_else(|| lanes.low.pop_front())
                    {
                        break job;
                    }
                    self.ready.wait(&mut lanes);
                }
            };
            // A panic in one decode must not take a worker down with it.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
        }
    }
}

/// Renditions that couldn't be made this session (a damaged file, a movie
/// Windows has no codec for), by photo and version, so scrolling past them
/// again doesn't read the file again.
static FAILED: Mutex<Option<HashSet<(i64, i64)>>> = Mutex::new(None);

fn failed_before(id: i64, mtime: i64) -> bool {
    FAILED
        .lock()
        .as_ref()
        .is_some_and(|f| f.contains(&(id, mtime)))
}

fn remember_failure(id: i64, mtime: i64) {
    FAILED.lock().get_or_insert_default().insert((id, mtime));
}

/// Gives every rendition another chance (after "Rebuild thumbnails").
pub fn forget_failures() {
    *FAILED.lock() = None;
}

pub fn lane_for(request: &Request<Vec<u8>>) -> Lane {
    match request.uri().path().as_bytes().get(1) {
        Some(b't') => Lane::Normal,
        _ => Lane::High,
    }
}

pub fn handle(state: &AppState, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let path = request.uri().path().trim_start_matches('/');
    let Some((kind, id)) = path.split_once('/') else {
        return status(StatusCode::NOT_FOUND);
    };
    let Ok(id) = id.parse::<i64>() else {
        return status(StatusCode::BAD_REQUEST);
    };
    let Ok((file, file_kind, size, mtime)) = state.lib.file_of(id) else {
        return status(StatusCode::NOT_FOUND);
    };
    let src = thumbs::Source {
        path: &file,
        kind: file_kind,
        size,
        mtime,
    };

    // While the Shield is off, serve what the cache holds and fail fast
    // otherwise, so the window shows a placeholder instead of waiting.
    if state.reach.covers(&file) {
        let wanted = if kind == "t" {
            Size::Thumb
        } else {
            Size::Preview
        };
        let other = if kind == "t" {
            Size::Preview
        } else {
            Size::Thumb
        };
        let exact = kind != "o" && state.cache.get(&src, wanted).is_some();
        return match state
            .cache
            .get(&src, wanted)
            .or_else(|| state.cache.get(&src, other))
            .and_then(|p| std::fs::read(p).ok())
        {
            Some(bytes) => image(bytes, "image/jpeg", exact),
            None => status(StatusCode::SERVICE_UNAVAILABLE),
        };
    }

    // Movies stream in pieces as the player asks for them.
    if kind == "o" && file_kind == Kind::Video {
        return movie(&file, request);
    }

    if kind != "o" && failed_before(id, mtime) {
        return status(StatusCode::UNPROCESSABLE_ENTITY);
    }

    let mut mime = "image/jpeg";
    let bytes = match kind {
        "t" => state
            .cache
            .ensure(&src, Size::Thumb)
            .and_then(|p| Ok(std::fs::read(p)?)),
        "p" => state
            .cache
            .ensure(&src, Size::Preview)
            .and_then(|p| Ok(std::fs::read(p)?)),
        "o" => match file_kind {
            Kind::Jpeg => std::fs::read(&file).map_err(Into::into).and_then(|bytes| {
                // A HEIC or PNG saved as .jpg: show the rendition instead.
                if fuji_core::decode::disguise(&bytes).is_some() {
                    state
                        .cache
                        .ensure(&src, Size::Preview)
                        .and_then(|p| Ok(std::fs::read(p)?))
                } else {
                    Ok(bytes)
                }
            }),
            Kind::Raf => raf::read_embedded_jpeg(&file).map_err(Into::into),
            Kind::Png => {
                mime = "image/png";
                std::fs::read(&file).map_err(Into::into)
            }
            // The webview can't show HEIF, PSD, TIFF or raw DNG data; the
            // 2560px rendition stands in.
            Kind::Heif | Kind::Psd | Kind::Tiff | Kind::Dng | Kind::Video => state
                .cache
                .ensure(&src, Size::Preview)
                .and_then(|p| Ok(std::fs::read(p)?)),
        },
        _ => return status(StatusCode::NOT_FOUND),
    };

    match bytes {
        Ok(bytes) => image(bytes, mime, true),
        Err(fuji_core::Error::Unsupported(_)) => status(StatusCode::UNSUPPORTED_MEDIA_TYPE),
        Err(fuji_core::Error::Io(e)) => {
            // Possibly the network: worth trying again later.
            log::warn!("{path}: {e}");
            status(StatusCode::INTERNAL_SERVER_ERROR)
        }
        Err(e) => {
            log::warn!("{path}: {e}");
            if kind != "o" {
                remember_failure(id, mtime);
            }
            status(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// `immutable` is false for a stand-in (a thumbnail served for a preview
/// while offline), which mustn't be remembered for that URL.
fn image(bytes: Vec<u8>, mime: &str, immutable: bool) -> Response<Vec<u8>> {
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        // URLs carry the file's mtime, so a given URL never changes.
        .header(
            header::CACHE_CONTROL,
            if immutable {
                "public, max-age=31536000, immutable"
            } else {
                "no-store"
            },
        )
        // Lets the histogram read pixels from a canvas without tainting it.
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(bytes)
        .unwrap()
}

/// Answers a `<video>` range request with up to [`CHUNK`] bytes.
fn movie(file: &Path, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Ok(mut f) = File::open(file) else {
        return status(StatusCode::NOT_FOUND);
    };
    let len = f.metadata().map_or(0, |m| m.len());
    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map_or(Some((0, u64::MAX)), |r| byte_range(r, len));
    let Some((start, end)) = range.filter(|&(start, _)| start < len) else {
        return Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{len}"))
            .body(Vec::new())
            .unwrap();
    };
    let end = end.min(len - 1).min(start + CHUNK - 1);
    let mut body = vec![0; (end - start + 1) as usize];
    if f.seek(SeekFrom::Start(start))
        .and_then(|_| f.read_exact(&mut body))
        .is_err()
    {
        return status(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        // Chromium's demuxer reads QuickTime files served as MP4.
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"))
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(body)
        .unwrap()
}

/// Bytes handed to the player per request: enough to keep it busy, small
/// enough not to hold a multi-gigabyte clip in memory.
const CHUNK: u64 = 4 << 20;

/// `bytes=START-END`, `bytes=START-` or `bytes=-SUFFIX` → inclusive range.
fn byte_range(header: &str, len: u64) -> Option<(u64, u64)> {
    let spec = header
        .trim()
        .strip_prefix("bytes=")?
        .split(',')
        .next()?
        .trim();
    let (a, b) = spec.split_once('-')?;
    match (a.trim(), b.trim()) {
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            Some((len.saturating_sub(n), u64::MAX))
        }
        (start, "") => Some((start.parse().ok()?, u64::MAX)),
        (start, end) => {
            let (start, end) = (start.parse().ok()?, end.parse().ok()?);
            (start <= end).then_some((start, end))
        }
    }
}

fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .body(Vec::new())
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players_get_the_bytes_they_ask_for() {
        assert_eq!(byte_range("bytes=0-", 100), Some((0, u64::MAX)));
        assert_eq!(byte_range("bytes=10-19", 100), Some((10, 19)));
        assert_eq!(byte_range("bytes=-30", 100), Some((70, u64::MAX)));
        assert_eq!(byte_range("bytes=20-10", 100), None);
        assert_eq!(byte_range("items=0-1", 100), None);

        let path = std::env::temp_dir().join(format!("fuji-movie-{}.mov", std::process::id()));
        std::fs::write(&path, (0..=255u8).collect::<Vec<_>>()).unwrap();
        let ask = |range: &str| {
            let request = Request::builder()
                .uri("fuji://localhost/o/1")
                .header(header::RANGE, range)
                .body(Vec::new())
                .unwrap();
            movie(&path, &request)
        };
        let r = ask("bytes=250-");
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(r.headers()[header::CONTENT_RANGE], "bytes 250-255/256");
        assert_eq!(r.body(), &[250, 251, 252, 253, 254, 255]);
        assert_eq!(
            ask("bytes=300-").status(),
            StatusCode::RANGE_NOT_SATISFIABLE
        );
        std::fs::remove_file(&path).unwrap();
    }
}
