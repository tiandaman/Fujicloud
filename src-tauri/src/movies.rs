//! Movies the window can't play (iPhone HEVC), played at once: FFmpeg
//! re-encodes to H.264 and the window plays what's been made so far, a
//! second or so behind the converter. When it's done the whole movie is
//! kept in the cache, so next time it just plays.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use fuji_core::ffmpeg;
use parking_lot::Mutex;

/// Still converting.
pub const RUNNING: u8 = 0;
/// All of it is made, and the whole movie is in the cache (`/v/<id>`).
pub const KEPT: u8 = 1;
/// It couldn't be converted (or was stopped for another movie).
pub const FAILED: u8 = 2;
/// All of it is made, but it couldn't be kept for next time.
pub const DONE: u8 = 3;

/// Bytes handed to the window per request.
const CHUNK: u64 = 1 << 20;

pub struct Stream {
    pub id: i64,
    /// What FFmpeg has written so far.
    part: PathBuf,
    written: AtomicU64,
    state: AtomicU8,
    child: Mutex<Option<Child>>,
    stopped: AtomicBool,
    /// The viewer is still showing it.
    watched: AtomicBool,
}

impl Stream {
    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        if let Some(child) = self.child.lock().as_mut() {
            let _ = child.kill();
        }
    }
}

/// The one movie being converted for the viewer.
#[derive(Default)]
pub struct Streams(Mutex<Option<Arc<Stream>>>);

impl Streams {
    /// The stream for `id`, if it's being (or has been) made.
    fn of(&self, id: i64) -> Option<Arc<Stream>> {
        self.0.lock().clone().filter(|s| s.id == id)
    }

    /// True while `id` is being made, or is made and still held.
    pub fn has(&self, id: i64) -> bool {
        self.of(id)
            .is_some_and(|s| s.state.load(Ordering::Relaxed) != FAILED)
    }

    /// Starts converting `src` for the viewer, in place of whatever was
    /// being converted before. The finished movie goes to `dest`.
    pub fn start(&self, ffmpeg: PathBuf, id: i64, src: PathBuf, dest: PathBuf) {
        let part = dest.with_extension("stream.mp4");
        let stream = Arc::new(Stream {
            id,
            part: part.clone(),
            written: AtomicU64::new(0),
            state: AtomicU8::new(RUNNING),
            child: Mutex::new(None),
            stopped: AtomicBool::new(false),
            watched: AtomicBool::new(true),
        });
        if let Some(old) = self.0.lock().replace(stream.clone()) {
            old.stop();
            // Its own thread deletes what it wrote once FFmpeg has gone.
            if old.state.load(Ordering::Relaxed) != RUNNING {
                let _ = std::fs::remove_file(&old.part);
            }
        }
        std::thread::spawn(move || {
            let state = match convert(&ffmpeg, &stream, &src, &dest) {
                Ok(state) => state,
                Err(e) => {
                    if !stream.stopped.load(Ordering::Relaxed) {
                        log::warn!("playing {}: {e}", src.display());
                    }
                    FAILED
                }
            };
            stream.state.store(state, Ordering::Relaxed);
            if state == FAILED || !stream.watched.load(Ordering::Relaxed) {
                let _ = std::fs::remove_file(&stream.part);
            }
        });
    }

    /// The state of `id`'s stream, then up to a megabyte of it from
    /// `offset`. An empty answer while it's running means "ask again".
    pub fn read(&self, id: i64, offset: u64) -> Vec<u8> {
        let Some(stream) = self.of(id) else {
            return vec![FAILED];
        };
        // The state first: "done" then means every byte is there to read.
        let state = stream.state.load(Ordering::Relaxed);
        let len = stream.written.load(Ordering::Relaxed);
        let mut out = vec![state];
        if state != FAILED && offset < len {
            let n = (len - offset).min(CHUNK) as usize;
            out.resize(1 + n, 0);
            let read = File::open(&stream.part).and_then(|mut f| {
                f.seek(SeekFrom::Start(offset))?;
                f.read_exact(&mut out[1..])
            });
            if read.is_err() {
                return vec![FAILED];
            }
        }
        out
    }

    /// The viewer has moved on from `id`. A conversion under way carries
    /// on (the movie is then ready next time) unless another replaces it.
    pub fn leave(&self, id: i64) {
        let Some(stream) = self.of(id) else { return };
        stream.watched.store(false, Ordering::Relaxed);
        if stream.state.load(Ordering::Relaxed) != RUNNING {
            let _ = std::fs::remove_file(&stream.part);
            let mut slot = self.0.lock();
            if slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, &stream)) {
                *slot = None;
            }
        }
    }
}

/// Runs FFmpeg to the end, keeping what it writes where the window can
/// read it; with the graphics card's help first, without if that fails.
fn convert(ffmpeg: &Path, stream: &Stream, src: &Path, dest: &Path) -> Result<u8, String> {
    let err = |e: &dyn std::fmt::Display| e.to_string();
    std::fs::create_dir_all(dest.parent().unwrap_or(Path::new("."))).map_err(|e| err(&e))?;
    for hardware in [true, false] {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&stream.part)
            .map_err(|e| err(&e))?;
        let mut child = ffmpeg::stream(ffmpeg, src, hardware).map_err(|e| err(&e))?;
        let mut out = child.stdout.take();
        let stderr = child.stderr.take();
        *stream.child.lock() = Some(child);
        if stream.stopped.load(Ordering::Relaxed) {
            // Stopped between starting and being put where `stop` finds it.
            stream.stop();
        }
        let errors = std::thread::spawn(move || {
            let mut said = Vec::new();
            if let Some(mut e) = stderr {
                let _ = e.read_to_end(&mut said);
            }
            said
        });
        let mut buf = vec![0; 256 << 10];
        let mut wrote = Ok(());
        while let Some(pipe) = out.as_mut() {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    wrote = file.write_all(&buf[..n]);
                    if wrote.is_err() {
                        // A full disk: nothing more can be kept.
                        stream.stop();
                        break;
                    }
                    stream.written.fetch_add(n as u64, Ordering::Relaxed);
                }
            }
        }
        drop(out);
        let status = stream.child.lock().take().map(|mut c| c.wait());
        let said = errors.join().unwrap_or_default();
        if stream.stopped.load(Ordering::Relaxed) {
            wrote.map_err(|e| err(&e))?;
            return Err("stopped".to_owned());
        }
        if matches!(status, Some(Ok(s)) if s.success()) {
            drop(file);
            // The whole movie, for next time.
            let whole = dest.with_extension("whole.mp4");
            let kept = ffmpeg::remux(ffmpeg, &stream.part, &whole)
                .and_then(|()| Ok(std::fs::rename(&whole, dest)?));
            return Ok(match kept {
                Ok(()) => KEPT,
                Err(e) => {
                    let _ = std::fs::remove_file(&whole);
                    log::warn!("keeping the converted {}: {e}", src.display());
                    DONE
                }
            });
        }
        let why = ffmpeg::complaint(&said);
        // Nothing has reached the window yet, so it can start again.
        if hardware && stream.written.load(Ordering::Relaxed) == 0 {
            log::info!(
                "{}: not with the graphics card ({why}); trying without",
                src.display()
            );
            continue;
        }
        return Err(why);
    }
    Err("FFmpeg couldn't convert the movie".to_owned())
}

/// Clears out what a crash or a kill left half-made.
pub fn sweep(movies: &Path) {
    let Ok(dir) = std::fs::read_dir(movies) else {
        return;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with(".stream.mp4") || name.ends_with(".whole.mp4") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}
