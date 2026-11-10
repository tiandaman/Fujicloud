//! Reading the text in photos (signs, menus, labels, screenshots), offline,
//! so it can be searched for.
//!
//! PaddleOCR's PP-OCRv5 mobile models: one finds where text is, the other
//! reads each line (English and other Latin-alphabet text without accents,
//! Chinese and Japanese). Text regions are taken as upright boxes, which is
//! simple and suits most signs and screens; measured on Total-Text's street
//! photos (120 of them, at about 1,000 px across) it finds 85% of the words
//! written level, a little over half of those on a slant, and a quarter of
//! those on a curve. From a 512 px thumbnail it finds 71% of the level ones:
//! small writing needs the bigger preview.

use std::path::{Path, PathBuf};

use fast_image_resize::{self as fr, images::Image};
use ort::session::Session;
use ort::value::Tensor;
use parking_lot::Mutex;

use crate::Error;
use crate::decode::Rgb;
use crate::embed::{cpu_builder, download_files, environment, model_err};

/// Stored with each photo read.
pub const MODEL_ID: &str = "pp-ocrv5-mobile";
/// The two models and the reader's alphabet, for the UI.
pub const DOWNLOAD_MB: u32 = 21;

const BASE: &str = "https://huggingface.co/PaddlePaddle";
/// (what to fetch, the name it's kept under)
const FILES: [(&str, &str); 3] = [
    (
        "PP-OCRv5_mobile_det_onnx/resolve/main/inference.onnx",
        "det.onnx",
    ),
    (
        "PP-OCRv5_mobile_rec_onnx/resolve/main/inference.onnx",
        "rec.onnx",
    ),
    (
        "PP-OCRv5_mobile_rec_onnx/resolve/main/inference.yml",
        "rec.yml",
    ),
];

/// The picture is looked over at most this many pixels across.
const LOOK: u32 = 960;
/// How sure a pixel must be to count as text, and a region on average.
const PIXEL: f32 = 0.3;
const REGION: f32 = 0.6;
/// A region is grown by its area × this ÷ its perimeter (the detector marks
/// the middle of each line).
const GROW: f32 = 1.5;
/// A line is read 48 pixels tall, and at most this wide.
const LINE_HEIGHT: u32 = 48;
const LINE_WIDTH: u32 = 1600;
/// Lines read less surely than this (on average over their letters) are
/// more often noise than writing.
const SURE: f32 = 0.6;
/// At most this many lines of a photo are read (a page of small print is
/// hundreds; what's searched for is in the biggest ones).
const MOST_LINES: usize = 60;

fn dir_for(dir: &Path) -> PathBuf {
    dir.join("text")
}

pub fn is_downloaded(dir: &Path) -> bool {
    FILES.iter().all(|(_, f)| dir_for(dir).join(f).is_file())
}

/// Fetches the models (~21 MB). `progress(done, total)` in bytes.
pub fn download(dir: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), Error> {
    let urls: Vec<String> = FILES.iter().map(|(u, _)| format!("{BASE}/{u}")).collect();
    let files: Vec<(&str, PathBuf)> = urls
        .iter()
        .zip(FILES)
        .map(|(u, (_, f))| (u.as_str(), dir_for(dir).join(f)))
        .collect();
    download_files(&files, progress)
}

pub struct Reader {
    finder: Mutex<Session>,
    reader: Mutex<Session>,
    /// What each of the reader's outputs stands for: nothing (the gap
    /// between letters) first, a space last.
    letters: Vec<String>,
}

/// The reader's alphabet, from the model's settings file: the items of its
/// `character_dict` list.
fn letters(yml: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut inside = false;
    for line in yml.lines() {
        if line.trim_end() == "  character_dict:" {
            inside = true;
        } else if inside {
            let Some(item) = line.strip_prefix("  - ") else {
                break;
            };
            let item = item.trim_end_matches(['\r', '\n']);
            let quoted = |q: char| item.len() >= 2 && item.starts_with(q) && item.ends_with(q);
            out.push(if quoted('\'') {
                item[1..item.len() - 1].replace("''", "'")
            } else if quoted('"') {
                item[1..item.len() - 1]
                    .replace("\\\\", "\u{0}")
                    .replace("\\\"", "\"")
                    .replace('\u{0}', "\\")
            } else {
                item.to_owned()
            });
        }
    }
    out.push(" ".to_owned());
    out
}

/// A region of the picture taken to hold a line of text, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Region {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

/// The regions of a `w` × `h` map of how likely each pixel is text: each
/// connected patch of likely pixels that's sure enough on average, grown,
/// biggest first.
fn regions(map: &[f32], w: usize, h: usize) -> Vec<Region> {
    let mut seen = vec![false; w * h];
    let mut out: Vec<(Region, usize)> = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if seen[start] || map[start] <= PIXEL {
            continue;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        let (mut sum, mut n) = (0f32, 0usize);
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            sum += map[i];
            n += 1;
            let mut visit = |j: usize| {
                if !seen[j] && map[j] > PIXEL {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x > 0 {
                visit(i - 1);
            }
            if x + 1 < w {
                visit(i + 1);
            }
            if y > 0 {
                visit(i - w);
            }
            if y + 1 < h {
                visit(i + w);
            }
        }
        let (bw, bh) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
        if bw.min(bh) < 3.0 || sum / (n as f32) < REGION {
            continue;
        }
        let grow = bw * bh * GROW / (2.0 * (bw + bh));
        let lo = |v: usize| (v as f32 - grow).max(0.0) as u32;
        let hi = |v: usize, most: usize| ((v as f32 + 1.0 + grow).ceil() as usize).min(most) as u32;
        out.push((
            Region {
                x0: lo(x0),
                y0: lo(y0),
                x1: hi(x1, w),
                y1: hi(y1, h),
            },
            n,
        ));
    }
    out.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    out.into_iter().map(|(r, _)| r).collect()
}

fn resized(image: &Rgb, region: Option<Region>, w: u32, h: u32) -> Result<Image<'static>, Error> {
    let src = Image::from_vec_u8(
        image.width,
        image.height,
        image.data.clone(),
        fr::PixelType::U8x3,
    )
    .map_err(model_err)?;
    let mut dst = Image::new(w, h, fr::PixelType::U8x3);
    let mut options =
        fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Bilinear));
    if let Some(r) = region {
        options = options.crop(
            r.x0 as f64,
            r.y0 as f64,
            (r.x1 - r.x0) as f64,
            (r.y1 - r.y0) as f64,
        );
    }
    fr::Resizer::new()
        .resize(&src, &mut dst, &options)
        .map_err(model_err)?;
    Ok(dst)
}

/// A picture as the models take it: blue, green, red planes, each value
/// with `mean` taken away and divided by `std`.
fn planes(image: &Image, mean: [f32; 3], std: [f32; 3]) -> Vec<f32> {
    let plane = (image.width() * image.height()) as usize;
    let mut out = vec![0f32; 3 * plane];
    for (i, px) in image.buffer().as_chunks::<3>().0.iter().enumerate() {
        for c in 0..3 {
            // The models were trained on blue-green-red pictures.
            out[c * plane + i] = (px[2 - c] as f32 / 255.0 - mean[c]) / std[c];
        }
    }
    out
}

/// A quarter turn anticlockwise (for writing that runs down the picture).
fn turned(image: &Rgb, r: Region) -> Rgb {
    let (w, h) = ((r.x1 - r.x0) as usize, (r.y1 - r.y0) as usize);
    let mut data = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let from = ((r.y0 as usize + y) * image.width as usize + r.x0 as usize + x) * 3;
            // (x, y) goes to column y, row w - 1 - x.
            let to = ((w - 1 - x) * h + y) * 3;
            data[to..to + 3].copy_from_slice(&image.data[from..from + 3]);
        }
    }
    Rgb {
        data,
        width: h as u32,
        height: w as u32,
    }
}

impl Reader {
    pub fn load(dir: &Path, threads: usize) -> Result<Self, Error> {
        let env = environment()?;
        let session = |name: &str| -> Result<Session, Error> {
            let file = dir_for(dir).join(name);
            let fail = |e: &dyn std::fmt::Display| Error::Model(format!("{}: {e}", file.display()));
            let builder = Session::builder(&env).map_err(|e| fail(&e))?;
            let mut builder = cpu_builder(builder, threads).map_err(|e| fail(&e))?;
            builder.commit_from_file(&file).map_err(|e| fail(&e))
        };
        let letters = letters(&std::fs::read_to_string(dir_for(dir).join("rec.yml"))?);
        if letters.len() < 100 {
            return Err(Error::Model("the text reader's alphabet is missing".into()));
        }
        Ok(Self {
            finder: Mutex::new(session("det.onnx")?),
            reader: Mutex::new(session("rec.onnx")?),
            letters,
        })
    }

    /// Where the lines of text are in an upright picture, biggest first.
    fn find(&self, image: &Rgb) -> Result<Vec<Region>, Error> {
        // No bigger than it is, and a multiple of 32 each way.
        let scale = (LOOK as f32 / image.width.max(image.height) as f32).min(1.0);
        let side = |v: u32| (((v as f32 * scale / 32.0).round() as u32) * 32).max(32);
        let (w, h) = (side(image.width), side(image.height));
        let input = planes(
            &resized(image, None, w, h)?,
            [0.485, 0.456, 0.406],
            [0.229, 0.224, 0.225],
        );
        let tensor =
            Tensor::from_array(([1usize, 3, h as usize, w as usize], input)).map_err(model_err)?;
        let mut session = self.finder.lock();
        let name = session.inputs()[0].name().to_owned();
        let outputs = session
            .run(ort::inputs![name => tensor])
            .map_err(model_err)?;
        let (_, map) = outputs[0].try_extract_tensor::<f32>().map_err(model_err)?;
        if map.len() != (w * h) as usize {
            return Err(Error::Model(
                "the text finder answered in another shape".into(),
            ));
        }
        let (sx, sy) = (
            image.width as f32 / w as f32,
            image.height as f32 / h as f32,
        );
        Ok(regions(map, w as usize, h as usize)
            .into_iter()
            .map(|r| Region {
                x0: (r.x0 as f32 * sx) as u32,
                y0: (r.y0 as f32 * sy) as u32,
                x1: ((r.x1 as f32 * sx).ceil() as u32).min(image.width),
                y1: ((r.y1 as f32 * sy).ceil() as u32).min(image.height),
            })
            .filter(|r| r.x1 > r.x0 + 1 && r.y1 > r.y0 + 1)
            .collect())
    }

    /// The line of text in a region, if it reads as one.
    fn line(&self, image: &Rgb, region: Region) -> Result<Option<String>, Error> {
        let (rw, rh) = (region.x1 - region.x0, region.y1 - region.y0);
        // Much taller than wide: writing running down the picture.
        let standing = rh as f32 > rw as f32 * 1.5;
        let (rw, rh) = if standing { (rh, rw) } else { (rw, rh) };
        let w = ((rw as f32 * LINE_HEIGHT as f32 / rh as f32).round() as u32).clamp(16, LINE_WIDTH);
        let small = if standing {
            resized(&turned(image, region), None, w, LINE_HEIGHT)?
        } else {
            resized(image, Some(region), w, LINE_HEIGHT)?
        };
        let input = planes(&small, [0.5; 3], [0.5; 3]);
        let tensor = Tensor::from_array(([1usize, 3, LINE_HEIGHT as usize, w as usize], input))
            .map_err(model_err)?;
        let mut session = self.reader.lock();
        let name = session.inputs()[0].name().to_owned();
        let outputs = session
            .run(ort::inputs![name => tensor])
            .map_err(model_err)?;
        let (shape, data) = outputs[0].try_extract_tensor::<f32>().map_err(model_err)?;
        let classes = *shape.last().unwrap_or(&1) as usize;
        if classes != self.letters.len() {
            return Err(Error::Model(format!(
                "the text reader knows {classes} letters, its alphabet has {}",
                self.letters.len()
            )));
        }
        // The likeliest letter at each step; repeats and gaps drop out.
        let mut text = String::new();
        let (mut sure, mut n, mut last) = (0f32, 0usize, 0usize);
        for step in data.chunks_exact(classes) {
            let (best, p) = step
                .iter()
                .enumerate()
                .fold((0, f32::MIN), |a, (i, &p)| if p > a.1 { (i, p) } else { a });
            if best != 0 && best != last {
                text.push_str(&self.letters[best]);
                sure += p;
                n += 1;
            }
            last = best;
        }
        let text = text.trim();
        Ok((n > 0 && sure / n as f32 >= SURE && !text.is_empty()).then(|| text.to_owned()))
    }

    /// The text in an upright picture: its lines, from the top down.
    pub fn read(&self, image: &Rgb) -> Result<Vec<String>, Error> {
        let mut regions = self.find(image)?;
        regions.truncate(MOST_LINES);
        // Top to bottom (in bands, so one line's words stay in order), then
        // left to right.
        let band = (image.height / 40).max(8);
        regions.sort_by_key(|r| (r.y0 / band, r.x0));
        let mut lines = Vec::new();
        for region in regions {
            if let Some(line) = self.line(image, region)? {
                lines.push(line);
            }
        }
        Ok(lines)
    }
}

pub use crate::written::searchable;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_regions_of_likely_text() {
        // A 40 × 20 map with a sure 20 × 4 line, a faint patch, and a speck.
        let (w, h) = (40, 20);
        let mut map = vec![0f32; w * h];
        for y in 6..10 {
            for x in 10..30 {
                map[y * w + x] = 0.9;
            }
        }
        for y in 14..18 {
            for x in 2..8 {
                map[y * w + x] = 0.4; // likely pixels, but not sure enough together
            }
        }
        map[w + 1] = 0.95;
        let found = regions(&map, w, h);
        assert_eq!(found.len(), 1, "{found:?}");
        // Grown by 20 * 4 * 1.5 / 48 = 2.5 each way.
        assert_eq!(
            found[0],
            Region {
                x0: 7,
                y0: 3,
                x1: 33,
                y1: 13
            }
        );
    }

    #[test]
    fn reads_the_alphabet_and_makes_text_searchable() {
        let yml = "PostProcess:\n  name: CTCLabelDecode\n  character_dict:\n  - a\n  - ''''\n  - \"\\\\\"\n  - '!'\n  - 一\nOther:\n  - x\n";
        assert_eq!(letters(yml), ["", "a", "'", "\\", "!", "一", " "]);
        assert_eq!(searchable("ハリファックス 港"), "ハリファックス 港");
    }

    #[test]
    fn turns_standing_writing_level() {
        // 2 wide, 3 tall: pixels numbered by their red value.
        let image = Rgb {
            data: (0..6u8).flat_map(|i| [i, 0, 0]).collect(),
            width: 2,
            height: 3,
        };
        let t = turned(
            &image,
            Region {
                x0: 0,
                y0: 0,
                x1: 2,
                y1: 3,
            },
        );
        assert_eq!((t.width, t.height), (3, 2));
        let reds: Vec<u8> = t.data.chunks(3).map(|p| p[0]).collect();
        // The right-hand column (1, 3, 5) is now the top row.
        assert_eq!(reds, [1, 3, 5, 0, 2, 4]);
    }

    fn model_dir() -> PathBuf {
        std::env::var("FUJICLOUD_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("fujicloud-models"))
    }

    /// The real models read a sign drawn here: big block letters made of
    /// squares, dark on light. Downloads 21 MB, so only when asked.
    #[test]
    #[ignore]
    fn real_text_is_read() {
        let dir = model_dir();
        download(&dir, &|_, _| {}).expect("download");
        let reader = Reader::load(&dir, 4).expect("load");
        // 5 × 7 letters.
        let font: [(char, [u8; 7]); 8] = [
            ('O', [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e]),
            ('P', [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10]),
            ('E', [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f]),
            ('N', [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11]),
            ('C', [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e]),
            ('A', [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11]),
            ('F', [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10]),
            (' ', [0; 7]),
        ];
        let (w, h, cell) = (960usize, 320usize, 14usize);
        let mut data = vec![235u8; w * h * 3];
        for (n, ch) in "OPEN CAFE".chars().enumerate() {
            let rows = font.iter().find(|(c, _)| *c == ch).unwrap().1;
            for (ry, row) in rows.iter().enumerate() {
                for rx in 0..5 {
                    if row & (0x10 >> rx) != 0 {
                        for y in 0..cell {
                            for x in 0..cell {
                                let (px, py) =
                                    (60 + n * 6 * cell + rx * cell + x, 110 + ry * cell + y);
                                data[(py * w + px) * 3..][..3].copy_from_slice(&[25, 25, 30]);
                            }
                        }
                    }
                }
            }
        }
        let sign = Rgb {
            data,
            width: w as u32,
            height: h as u32,
        };
        let lines = reader.read(&sign).unwrap();
        println!("{lines:?}");
        assert_eq!(searchable(&lines.join(" ")), "open cafe");
        // Nothing written, nothing read.
        let blank = Rgb {
            data: vec![120u8; 640 * 480 * 3],
            width: 640,
            height: 480,
        };
        assert!(reader.read(&blank).unwrap().is_empty());
    }
}
