//! Times the thumbnail/preview pipeline on a camera-sized JPEG.
//!
//!     cargo run --release -p fuji-core --example thumb_bench [path/to/photo.jpg]

use std::time::Instant;

use fuji_core::thumbs::{self, Size};

fn main() {
    let jpeg = match std::env::args().nth(1) {
        Some(path) => std::fs::read(path).expect("read photo"),
        // Same pixel count as an X-T5 / X-S20 26MP frame.
        None => fuji_core::test_jpeg(6240, 4160, Some("2026:10:21 09:15:30")),
    };
    println!("source: {:.1} MB", jpeg.len() as f64 / 1e6);

    for (label, size) in [("thumbnail", Size::Thumb), ("preview", Size::Preview)] {
        let runs = 10;
        let start = Instant::now();
        let mut out = None;
        for _ in 0..runs {
            out = Some(thumbs::render(&jpeg, size.long_edge(), 82, true).expect("render"));
        }
        let out = out.unwrap();
        println!(
            "{label:>9}: {:>6.1} ms  ({}x{}, {} KB)",
            start.elapsed().as_secs_f64() * 1000.0 / runs as f64,
            out.width,
            out.height,
            out.jpeg.len() / 1024
        );
    }
}
