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

use std::collections::VecDeque;
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
            Kind::Jpeg => std::fs::read(&file).map_err(Into::into),
            Kind::Raf => raf::read_embedded_jpeg(&file).map_err(Into::into),
            // The webview can't show PSD/TIFF; the 2560px rendition stands in.
            Kind::Psd | Kind::Tiff => state
                .cache
                .ensure(&src, Size::Preview)
                .and_then(|p| Ok(std::fs::read(p)?)),
            other => Err(fuji_core::Error::Unsupported(other)),
        },
        _ => return status(StatusCode::NOT_FOUND),
    };

    match bytes {
        Ok(bytes) => Response::builder()
            .header(header::CONTENT_TYPE, "image/jpeg")
            // URLs carry the file's mtime, so a given URL never changes.
            .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
            // Lets the histogram read pixels from a canvas without tainting it.
            .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
            .body(bytes)
            .unwrap(),
        Err(fuji_core::Error::Unsupported(_)) => status(StatusCode::UNSUPPORTED_MEDIA_TYPE),
        Err(e) => {
            log::warn!("{path}: {e}");
            status(StatusCode::INTERNAL_SERVER_ERROR)
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
