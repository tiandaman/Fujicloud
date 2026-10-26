//! Makes a thumbnail of every photo under the given folders, exactly as the
//! app does, and lists the ones that fail. For tracking down "this photo has
//! no thumbnail" reports.
//!
//!     cargo run --release -p fuji-core --example thumb_check -- <folder>... [--out <folder>]
//!
//! With `--out`, the thumbnails are kept there to look at.

use std::path::{Path, PathBuf};
use std::time::Instant;

use fuji_core::Kind;
use fuji_core::thumbs::{Cache, Size, Source};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if Kind::from_path(&p).is_some() {
            out.push(p);
        }
    }
}

fn main() {
    // On a thread like the app's image workers (Rust's default 2 MB stack),
    // not the bigger main thread: a decoder that needs more stack must show
    // up here, since in the app it takes the whole program down.
    std::thread::Builder::new()
        .name("fuji-img-check".into())
        .spawn(check)
        .expect("spawn")
        .join()
        .expect("thumb_check crashed");
}

fn check() {
    let mut args = std::env::args().skip(1);
    let (mut dirs, mut keep) = (Vec::new(), None);
    while let Some(a) = args.next() {
        if a == "--out" {
            keep = args.next().map(PathBuf::from);
        } else {
            dirs.push(PathBuf::from(a));
        }
    }
    let scratch =
        std::env::temp_dir().join(format!("fujicloud-thumb-check-{}", std::process::id()));
    let root = keep.clone().unwrap_or(scratch.clone());
    let cache = Cache::new(&root).expect("cache folder");
    let mut files = Vec::new();
    for d in &dirs {
        walk(d, &mut files);
    }
    files.sort();
    let (mut ok, mut failed) = (0, 0);
    for path in &files {
        let meta = std::fs::metadata(path).unwrap();
        let src = Source {
            path,
            kind: Kind::from_path(path).unwrap(),
            size: meta.len(),
            mtime: 0,
        };
        let start = Instant::now();
        match cache.ensure(&src, Size::Thumb) {
            Ok(thumb) => {
                ok += 1;
                let jpeg = std::fs::read(&thumb).unwrap();
                let (w, h) = fuji_core::thumbs::decode_rendition(&jpeg, 4096)
                    .map(|i| (i.width, i.height))
                    .unwrap_or((0, 0));
                println!(
                    "ok    {:>5} ms  {w}x{h}  {}  -> {}",
                    start.elapsed().as_millis(),
                    path.display(),
                    thumb.display()
                );
            }
            Err(e) => {
                failed += 1;
                println!(
                    "FAIL  {:>5} ms  {}: {e}",
                    start.elapsed().as_millis(),
                    path.display()
                );
            }
        }
    }
    println!("{ok} ok, {failed} failed");
    if keep.is_none() {
        let _ = std::fs::remove_dir_all(scratch);
    }
}
