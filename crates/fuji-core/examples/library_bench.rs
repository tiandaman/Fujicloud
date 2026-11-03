//! Times what a first run does with a library on this PC, CPU only: the
//! scan, thumbnails and previews, then smart search, faces and pets, each
//! alone and all at once (as the app runs them), and how long opening a
//! photo takes meanwhile. Downloads the models into `models` if missing.
//!
//!     cargo run --release -p fuji-core --example library_bench -- <photos> <models> [threads]
//!
//! `threads` is the cores to assume (default: all); the analyses use the
//! share of them the app gives each.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fuji_core::animals::{Detector, Identifier};
use fuji_core::embed::{Device, Engine, SIGLIP_BASE};
use fuji_core::faces::FaceModels;
use fuji_core::thumbs::{self, Cache, Size, Source};
use fuji_core::{Kind, Library, scan};

fn memory() -> String {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|kb| kb.parse::<u64>().ok())
            .map_or("?".to_owned(), |kb| format!("{} MB", kb / 1024))
    };
    format!("memory now {}, peak {}", field("VmRSS:"), field("VmHWM:"))
}

fn per(total: Duration, n: usize) -> String {
    format!(
        "{:.0} ms each",
        total.as_secs_f64() * 1000.0 / n.max(1) as f64
    )
}

struct Photo {
    path: PathBuf,
    kind: Kind,
    size: u64,
    mtime: i64,
}

impl Photo {
    fn source(&self) -> Source<'_> {
        Source {
            path: &self.path,
            kind: self.kind,
            size: self.size,
            mtime: self.mtime,
        }
    }
}

fn picture(cache: &Cache, p: &Photo) -> fuji_core::decode::Rgb {
    let file = cache
        .get(&p.source(), Size::Preview)
        .or_else(|| cache.get(&p.source(), Size::Thumb))
        .expect("rendition");
    thumbs::decode_rendition(&std::fs::read(file).unwrap(), 1280).unwrap()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let photos_dir = PathBuf::from(&args[1]);
    let models = PathBuf::from(&args[2]);
    let cores = args
        .get(3)
        .and_then(|t| t.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()));
    // As the app shares them out.
    let (smart_threads, face_threads, pet_threads) =
        (cores / 2, (cores / 4).max(1), (cores / 4).max(1));
    println!(
        "{cores} cores: smart search {smart_threads} threads, faces {face_threads}, pets {pet_threads}"
    );

    let work = tempfile::tempdir().unwrap();
    let lib = Library::open(&work.path().join("library.db")).unwrap();
    let cache = Cache::new(work.path().join("cache")).unwrap();
    let src = lib.add_source(&photos_dir).unwrap();
    let start = Instant::now();
    let summary = scan::scan_source(&lib, &src, false, &|_| {}).unwrap();
    let mut photos: Vec<Photo> = lib
        .photos()
        .unwrap()
        .into_iter()
        .map(|r| {
            let (path, kind, size, mtime) = lib.file_of(r.id).unwrap();
            Photo {
                path,
                kind,
                size,
                mtime,
            }
        })
        .collect();
    photos.sort_by(|a, b| a.path.cmp(&b.path));
    println!(
        "scan: {} photos in {:.2} s ({})",
        summary.added,
        start.elapsed().as_secs_f64(),
        per(start.elapsed(), photos.len())
    );

    // Thumbnails and previews, one photo at a time, by kind.
    let mut by_kind: Vec<(String, Duration, usize)> = Vec::new();
    let start = Instant::now();
    for p in &photos {
        let t = Instant::now();
        cache.ensure(&p.source(), Size::Thumb).unwrap();
        cache.ensure(&p.source(), Size::Preview).unwrap();
        let label = format!("{:?}", p.kind);
        match by_kind.iter_mut().find(|k| k.0 == label) {
            Some(k) => (k.1, k.2) = (k.1 + t.elapsed(), k.2 + 1),
            None => by_kind.push((label, t.elapsed(), 1)),
        }
    }
    println!(
        "thumbnail + preview, one at a time: {:.1} s ({})",
        start.elapsed().as_secs_f64(),
        per(start.elapsed(), photos.len())
    );
    for (kind, total, n) in &by_kind {
        println!("    {kind:>6}: {n:>3} photos, {}", per(*total, *n));
    }

    // Smart search.
    SIGLIP_BASE
        .download(&models, Device::Cpu, &|_, _| {})
        .expect("download smart search");
    let thumbs: Vec<Vec<u8>> = photos
        .iter()
        .map(|p| std::fs::read(cache.get(&p.source(), Size::Thumb).unwrap()).unwrap())
        .collect();
    for threads in [smart_threads, cores] {
        let engine = Engine::load(&models, &SIGLIP_BASE, Device::Cpu, threads).unwrap();
        let start = Instant::now();
        for chunk in thumbs.chunks(engine.batch_size()) {
            engine.embed_images(chunk).unwrap();
        }
        println!(
            "smart search, {threads} threads: {}; {}",
            per(start.elapsed(), thumbs.len()),
            memory()
        );
    }

    // Faces.
    fuji_core::faces::download(&models, &|_, _| {}).expect("download faces");
    let pictures: Vec<_> = photos.iter().map(|p| picture(&cache, p)).collect();
    for threads in [face_threads, cores] {
        let faces = FaceModels::load(&models, threads).unwrap();
        let start = Instant::now();
        let mut found = 0;
        for image in &pictures {
            found += faces.faces(image).unwrap().len();
        }
        println!(
            "faces, {threads} threads: {} ({found} faces); {}",
            per(start.elapsed(), pictures.len()),
            memory()
        );
    }

    // Pets.
    fuji_core::animals::download(&models, &|_, _| {}).expect("download pets");
    for threads in [pet_threads, cores] {
        let detector = Detector::load(&models, threads).unwrap();
        let identifier = Identifier::load(&models, threads).unwrap();
        let start = Instant::now();
        let mut found = 0;
        for image in &pictures {
            for animal in detector.animals(image).unwrap() {
                identifier.pet(image, &animal).unwrap();
                found += 1;
            }
        }
        println!(
            "pets, {threads} threads: {} ({found} pets); {}",
            per(start.elapsed(), pictures.len()),
            memory()
        );
    }

    // Everything at once, as on a first run, while someone opens photos,
    // with two ways of sharing the processor out.
    let open_photo = |p: &Photo| {
        let t = Instant::now();
        let file = cache.get(&p.source(), Size::Preview).unwrap();
        thumbs::decode_rendition(&std::fs::read(file).unwrap(), 2560).unwrap();
        t.elapsed()
    };
    let idle: Duration = photos.iter().take(20).map(open_photo).sum::<Duration>() / 20;
    println!(
        "opening a photo when idle: {:.0} ms",
        idle.as_secs_f64() * 1000.0
    );
    let faces_first = [(cores / 4).max(1), (cores / 2).max(1), (cores / 4).max(1)];
    for (label, threads) in [
        (
            "smart search's share bigger",
            [smart_threads, face_threads, pet_threads],
        ),
        ("faces' share bigger", faces_first),
    ] {
        let engine = Engine::load(&models, &SIGLIP_BASE, Device::Cpu, threads[0]).unwrap();
        let faces = FaceModels::load(&models, threads[1]).unwrap();
        let detector = Detector::load(&models, threads[2]).unwrap();
        let identifier = Identifier::load(&models, threads[2]).unwrap();
        println!("{label} {threads:?}: all models loaded, {}", memory());
        let busy = AtomicBool::new(true);
        let start = Instant::now();
        let took = std::thread::scope(|s| {
            let jobs = [
                s.spawn(|| {
                    for chunk in thumbs.chunks(engine.batch_size()) {
                        engine.embed_images(chunk).unwrap();
                    }
                    start.elapsed()
                }),
                s.spawn(|| {
                    for image in &pictures {
                        faces.faces(image).unwrap();
                    }
                    start.elapsed()
                }),
                s.spawn(|| {
                    for image in &pictures {
                        for animal in detector.animals(image).unwrap() {
                            identifier.pet(image, &animal).unwrap();
                        }
                    }
                    start.elapsed()
                }),
            ];
            let viewer = s.spawn(|| {
                let mut times = Vec::new();
                while busy.load(Ordering::Relaxed) {
                    for p in photos.iter().take(20) {
                        times.push(open_photo(p));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                times
            });
            let took: Vec<Duration> = jobs.into_iter().map(|j| j.join().unwrap()).collect();
            busy.store(false, Ordering::Relaxed);
            let mut times = viewer.join().unwrap();
            times.sort();
            println!(
                "    opening a photo meanwhile: {:.0} ms (median), {:.0} ms (worst 10%)",
                times[times.len() / 2].as_secs_f64() * 1000.0,
                times[times.len() * 9 / 10].as_secs_f64() * 1000.0
            );
            took
        });
        println!(
            "    done: smart search {:.1} s, faces {:.1} s, pets {:.1} s for {} photos ({} for all three); {}",
            took[0].as_secs_f64(),
            took[1].as_secs_f64(),
            took[2].as_secs_f64(),
            photos.len(),
            per(*took.iter().max().unwrap(), photos.len()),
            memory()
        );
    }
}
